#!/usr/bin/env node
// Validate the Jeliya OKF documentation profile.
//
// This deliberately parses a small, explicit YAML subset instead of loading
// arbitrary YAML: frontmatter values may only be strings or string arrays.
// Anchors, aliases, tags, maps, implicit types, and duplicate keys are
// rejected. That keeps the docs format deterministic and makes validation
// safe for pull requests from untrusted contributors.
//
// Run: node scripts/check-docs.mjs

import { existsSync, lstatSync, realpathSync, readFileSync, readdirSync } from 'node:fs';
import { dirname, extname, isAbsolute, relative, resolve, sep } from 'node:path';
import { TextDecoder } from 'node:util';
import { fileURLToPath } from 'node:url';

const SCRIPT_PATH = fileURLToPath(import.meta.url);
const DEFAULT_REPO_ROOT = resolve(dirname(SCRIPT_PATH), '..');

export const PROFILE = Object.freeze({
  requiredFields: Object.freeze([
    'type',
    'title',
    'description',
    'tags',
    'timestamp',
    'status',
    'implementation_status',
    'verification_status',
    'release_status',
    'audience',
  ]),
  types: Object.freeze([
    'Architecture',
    'Reference',
    'Guide',
    'Runbook',
    'Decision',
    'Policy',
    'Research',
    'Status Report',
    'Glossary',
  ]),
  statuses: Object.freeze(['draft', 'proposal', 'canonical', 'deprecated']),
  implementationStatuses: Object.freeze([
    'not-applicable',
    'planned',
    'partial',
    'implemented',
  ]),
  verificationStatuses: Object.freeze([
    'not-applicable',
    'unverified',
    'partial',
    'verified',
    'historical',
  ]),
  releaseStatuses: Object.freeze([
    'not-applicable',
    'unreleased',
    'partial',
    'released',
  ]),
});

const FORBIDDEN_KEYS = new Set(['__proto__', 'constructor', 'prototype']);
const URI_SCHEME = /^[A-Za-z][A-Za-z0-9+.-]*:/;
const UTC_TIMESTAMP =
  /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.(\d{1,3}))?Z$/;

function compareText(a, b) {
  return a < b ? -1 : a > b ? 1 : 0;
}

function toRepoPath(repoRoot, path) {
  return relative(repoRoot, path).split(sep).join('/');
}

function isWithin(root, path) {
  const rel = relative(root, path);
  return rel === '' || (!isAbsolute(rel) && rel !== '..' && !rel.startsWith(`..${sep}`));
}

function hasSymlinkComponent(root, path) {
  const rel = relative(root, path);
  if (!isWithin(root, path)) return false;
  let current = root;
  for (const part of rel.split(sep).filter(Boolean)) {
    current = resolve(current, part);
    if (lstatSync(current).isSymbolicLink()) return true;
  }
  return false;
}

function issue(file, line, code, message) {
  return { file, line, code, message };
}

function readUtf8Document(path, file, findings) {
  const bytes = readFileSync(path);
  try {
    return new TextDecoder('utf-8', { fatal: true }).decode(bytes);
  } catch {
    findings.push(
      issue(file, 1, 'encoding-utf8', 'Markdown document must contain valid UTF-8'),
    );
    // Content has no reliable character boundaries, so do not manufacture
    // secondary metadata, heading, or link findings from lossy replacement.
    return null;
  }
}

function scalarError(value) {
  if (/^(?:[|>{}\[\]])/.test(value)) {
    return 'nested values and multiline YAML are not supported';
  }
  if (/^(?:&|\*|!)[A-Za-z0-9_-]+/.test(value)) {
    return 'YAML anchors, aliases, and tags are not allowed';
  }
  if (/(?:^|\s)(?:&|\*|!)[A-Za-z0-9_-]+/.test(value)) {
    return 'YAML anchors, aliases, and tags are not allowed';
  }
  return null;
}

function parseScalar(raw) {
  const value = raw.trim();
  if (value === '') throw new Error('value must not be empty');

  if (!value.startsWith('"')) {
    const unsupported = scalarError(value);
    if (unsupported) throw new Error(unsupported);
    throw new Error('strings must use double quotes');
  }
  if (!value.endsWith('"')) throw new Error('unterminated double-quoted string');
  let parsed;
  try {
    parsed = JSON.parse(value);
  } catch {
    throw new Error('invalid double-quoted string');
  }
  if (typeof parsed !== 'string') throw new Error('value must be a string');
  if (!parsed.isWellFormed()) throw new Error('string must contain valid Unicode scalar values');
  return parsed;
}

function splitFlowSequence(raw) {
  const inner = raw.slice(1, -1);
  if (inner.trim() === '') return [];

  const items = [];
  let start = 0;
  let quote = null;
  let escaped = false;
  for (let i = 0; i < inner.length; i += 1) {
    const char = inner[i];
    if (quote === '"') {
      if (escaped) escaped = false;
      else if (char === '\\') escaped = true;
      else if (char === '"') quote = null;
      continue;
    }
    if (char === '"') {
      quote = char;
    } else if (char === ',') {
      items.push(inner.slice(start, i));
      start = i + 1;
    } else if ('[]{}'.includes(char)) {
      throw new Error('nested YAML collections are not supported');
    }
  }
  if (quote) throw new Error('unterminated quoted string in array');
  items.push(inner.slice(start));
  return items.map(parseScalar);
}

function parseValue(raw) {
  const value = raw.trim();
  if (value.startsWith('[')) {
    if (!value.endsWith(']')) throw new Error('unterminated flow array');
    return splitFlowSequence(value);
  }
  return parseScalar(value);
}

/** Parse the restricted frontmatter format used by the Jeliya docs profile. */
export function parseFrontmatter(source, file = '<document>') {
  const normalized = source.replace(/^\uFEFF/, '').replace(/\r\n?/g, '\n');
  const lines = normalized.split('\n');
  if (lines[0] !== '---') {
    return {
      data: null,
      body: normalized,
      bodyStartLine: 1,
      errors: [],
    };
  }

  const close = lines.indexOf('---', 1);
  if (close === -1) {
    return {
      data: Object.create(null),
      body: '',
      bodyStartLine: lines.length + 1,
      errors: [
        issue(file, 1, 'frontmatter-unclosed', 'frontmatter has no closing --- delimiter'),
      ],
    };
  }

  const data = Object.create(null);
  const errors = [];
  const seenKeys = new Set();
  for (let index = 1; index < close; index += 1) {
    const rawLine = lines[index];
    const trimmed = rawLine.trim();
    if (trimmed === '' || trimmed.startsWith('#')) continue;
    if (/^\s/.test(rawLine)) {
      errors.push(
        issue(file, index + 1, 'frontmatter-indentation', 'unexpected indented value'),
      );
      continue;
    }

    const match = /^([A-Za-z][A-Za-z0-9_-]*):(?:[ \t]*(.*))?$/.exec(rawLine);
    if (!match) {
      errors.push(
        issue(file, index + 1, 'frontmatter-syntax', 'expected a key: value entry'),
      );
      continue;
    }
    const [, key, rawValue = ''] = match;
    if (FORBIDDEN_KEYS.has(key)) {
      errors.push(
        issue(file, index + 1, 'frontmatter-key', `unsafe key is not allowed: ${key}`),
      );
      continue;
    }
    if (seenKeys.has(key)) {
      errors.push(
        issue(file, index + 1, 'frontmatter-duplicate', `duplicate key: ${key}`),
      );
      continue;
    }
    seenKeys.add(key);

    if (rawValue.trim() === '') {
      errors.push(
        issue(
          file,
          index + 1,
          'frontmatter-value',
          `${key}: values must be double-quoted strings or flow-style string arrays`,
        ),
      );
      continue;
    }

    try {
      data[key] = parseValue(rawValue);
    } catch (error) {
      errors.push(issue(file, index + 1, 'frontmatter-value', `${key}: ${error.message}`));
    }
  }

  return {
    data,
    body: lines.slice(close + 1).join('\n'),
    bodyStartLine: close + 2,
    errors,
  };
}

function markdownFiles(dir, repoRoot, findings) {
  const files = [];
  for (const entry of readdirSync(dir, { withFileTypes: true }).sort((a, b) =>
    compareText(a.name, b.name),
  )) {
    const path = resolve(dir, entry.name);
    const file = toRepoPath(repoRoot, path);
    const lowerName = entry.name.toLowerCase();
    if ((lowerName === 'index.md' || lowerName === 'log.md') && entry.name !== lowerName) {
      findings.push(
        issue(
          file,
          1,
          'reserved-name-case',
          `reserved documentation filename must use exact lowercase spelling: ${lowerName}`,
        ),
      );
    }
    if (entry.isSymbolicLink()) {
      findings.push(
        issue(file, 1, 'docs-symlink', 'documentation entries must not be symbolic links'),
      );
    } else if (entry.isDirectory()) {
      if (entry.name.toLowerCase().endsWith('.md')) {
        findings.push(
          issue(file, 1, 'docs-file-type', 'Markdown documents must be regular files'),
        );
      } else {
        files.push(...markdownFiles(path, repoRoot, findings));
      }
    } else if (entry.isFile() && entry.name.toLowerCase().endsWith('.md')) {
      files.push(path);
    } else if (!entry.isFile() && entry.name.toLowerCase().endsWith('.md')) {
      findings.push(
        issue(file, 1, 'docs-file-type', 'Markdown documents must be regular files'),
      );
    }
  }
  return files;
}

function maskCharacters(text) {
  return text.replace(/[^\n]/g, ' ');
}

function maskMarkdownCode(source) {
  let masked = source.replace(/<!--[\s\S]*?-->/g, maskCharacters);
  const lines = masked.split('\n');
  let fence = null;
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    const marker = /^ {0,3}(`{3,}|~{3,})/.exec(line)?.[1];
    if (fence) {
      lines[index] = maskCharacters(line);
      if (marker && marker[0] === fence[0] && marker.length >= fence.length) fence = null;
    } else if (marker) {
      fence = marker;
      lines[index] = maskCharacters(line);
    } else if (/^(?: {4}|\t)/.test(line)) {
      lines[index] = maskCharacters(line);
    }
  }
  masked = lines.join('\n');
  return masked.replace(/(`+)[\s\S]*?\1/g, maskCharacters);
}

function lineAt(source, index, bodyStartLine) {
  let line = bodyStartLine;
  for (let cursor = 0; cursor < index; cursor += 1) {
    if (source.charCodeAt(cursor) === 10) line += 1;
  }
  return line;
}

function referenceDefinitions(source) {
  const definitions = new Map();
  const pattern = /^ {0,3}\[([^\]\n]+)\]:[ \t]*(?:<([^>\n]+)>|(\S+))/gm;
  for (const match of source.matchAll(pattern)) {
    const label = match[1].trim().replace(/\s+/g, ' ').toLowerCase();
    const destination = match[2] ?? match[3];
    if (!definitions.has(label)) definitions.set(label, destination);
  }
  return definitions;
}

function isEscaped(source, index) {
  let slashes = 0;
  for (let cursor = index - 1; cursor >= 0 && source[cursor] === '\\'; cursor -= 1) {
    slashes += 1;
  }
  return slashes % 2 === 1;
}

function isImageMarker(source, openBracket) {
  const marker = openBracket - 1;
  return marker >= 0 && source[marker] === '!' && !isEscaped(source, marker);
}

function openingBracket(source, closeBracket) {
  let nested = 0;
  for (let index = closeBracket - 1; index >= 0 && source[index] !== '\n'; index -= 1) {
    if (isEscaped(source, index)) continue;
    if (source[index] === ']') {
      nested += 1;
    } else if (source[index] === '[') {
      if (nested === 0) return index;
      nested -= 1;
    }
  }
  return -1;
}

function destinationFromLinkContents(contents) {
  const trimmed = contents.trim();
  if (trimmed === '') return '';
  if (trimmed.startsWith('<')) {
    const close = trimmed.indexOf('>');
    if (close === -1) return null;
    return trimmed.slice(1, close);
  }
  let escaped = false;
  for (let index = 0; index < trimmed.length; index += 1) {
    const char = trimmed[index];
    if (escaped) {
      escaped = false;
    } else if (char === '\\') {
      escaped = true;
    } else if (/\s/.test(char)) {
      return trimmed.slice(0, index);
    }
  }
  return trimmed;
}

function inlineLinks(source, bodyStartLine) {
  const links = [];
  for (let index = 0; index < source.length - 1; index += 1) {
    const open = source[index] === ']' ? openingBracket(source, index) : -1;
    if (
      source[index] !== ']' ||
      source[index + 1] !== '(' ||
      source[index - 1] === '\\' ||
      open === -1
    ) {
      continue;
    }
    let cursor = index + 2;
    let depth = 1;
    let quote = null;
    let escaped = false;
    for (; cursor < source.length; cursor += 1) {
      const char = source[cursor];
      if (escaped) {
        escaped = false;
        continue;
      }
      if (char === '\\') {
        escaped = true;
        continue;
      }
      if (quote) {
        if (char === quote) quote = null;
        continue;
      }
      if (char === '"' || char === "'") {
        quote = char;
      } else if (char === '(') {
        depth += 1;
      } else if (char === ')') {
        depth -= 1;
        if (depth === 0) break;
      }
    }
    if (depth !== 0) continue;
    const destination = destinationFromLinkContents(
      source.slice(index + 2, cursor),
    );
    if (destination !== null) {
      links.push({
        destination,
        line: lineAt(source, index, bodyStartLine),
        navigation: !isImageMarker(source, open),
      });
    }
    index = cursor;
  }
  return links;
}

function automaticLinks(source, bodyStartLine) {
  const links = [];
  const pattern = /<([A-Za-z][A-Za-z0-9+.-]*:[^<>\s]+|[^<>\s@]+@[^<>\s@]+)>/g;
  for (const match of source.matchAll(pattern)) {
    links.push({
      destination: match[1].includes('@') && !URI_SCHEME.test(match[1])
        ? `mailto:${match[1]}`
        : match[1],
      line: lineAt(source, match.index, bodyStartLine),
      navigation: false,
    });
  }
  return links;
}

function referenceLinks(source, bodyStartLine, definitions) {
  const links = [];
  const missing = [];
  const occupied = [];
  const pattern = /!?\[([^\]\n]+)\]\[([^\]\n]*)\]/g;
  for (const match of source.matchAll(pattern)) {
    const open = match.index + (match[0].startsWith('!') ? 1 : 0);
    if (isEscaped(source, open)) continue;
    if (
      /^ {0,3}\[[^\]]+\]:/.test(
        source.slice(source.lastIndexOf('\n', match.index) + 1),
      )
    ) {
      continue;
    }
    const label = (match[2] || match[1]).trim().replace(/\s+/g, ' ').toLowerCase();
    const line = lineAt(source, match.index, bodyStartLine);
    occupied.push([match.index, match.index + match[0].length]);
    if (!definitions.has(label)) {
      missing.push({ line, label });
      continue;
    }
    links.push({
      destination: definitions.get(label),
      line,
      navigation: !isImageMarker(source, open),
    });
  }

  const shortcutPattern = /!?\[([^\]\n]+)\](?![(:\[])/g;
  for (const match of source.matchAll(shortcutPattern)) {
    const open = match.index + (match[0].startsWith('!') ? 1 : 0);
    if (isEscaped(source, open)) continue;
    if (occupied.some(([start, end]) => match.index >= start && match.index < end)) continue;
    if (
      /^ {0,3}\[[^\]]+\]:/.test(
        source.slice(source.lastIndexOf('\n', match.index) + 1),
      )
    ) {
      continue;
    }
    const label = match[1].trim().replace(/\s+/g, ' ').toLowerCase();
    if (!definitions.has(label)) continue;
    links.push({
      destination: definitions.get(label),
      line: lineAt(source, match.index, bodyStartLine),
      navigation: !isImageMarker(source, open),
    });
  }
  return { links, missing };
}

function markdownLinks(body, bodyStartLine) {
  const source = maskMarkdownCode(body);
  const definitions = referenceDefinitions(source);
  const usages = referenceLinks(source, bodyStartLine, definitions);
  return {
    links: [
      ...inlineLinks(source, bodyStartLine),
      ...automaticLinks(source, bodyStartLine),
      ...usages.links,
    ],
    missingReferences: usages.missing,
  };
}

function headingText(raw) {
  return raw
    .replace(/\s+#+\s*$/, '')
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/\[([^\]]+)\]\([^)]*\)/g, '$1')
    .replace(/<[^>]+>/g, '')
    .replace(/[`*_~]/g, '')
    .replace(/\\([\\`*{}\[\]()#+.!_-])/g, '$1')
    .trim();
}

function githubSlug(value) {
  return value
    .toLocaleLowerCase('en-US')
    .replace(/[^\p{L}\p{N}\p{M}\s_-]/gu, '')
    .trim()
    .replace(/\s/g, '-');
}

function markdownAnchors(path, cache) {
  if (cache.has(path)) return cache.get(path);
  const source = readFileSync(path, 'utf8').replace(/\r\n?/g, '\n');
  const { body } = parseFrontmatter(source);
  const masked = maskMarkdownCode(body);
  const anchors = new Set();
  const seen = new Map();
  for (const match of masked.matchAll(/^ {0,3}#{1,6}[ \t]+(.+)$/gm)) {
    const base = githubSlug(headingText(match[1]));
    if (base === '') continue;
    const duplicate = seen.get(base) ?? 0;
    seen.set(base, duplicate + 1);
    anchors.add(duplicate === 0 ? base : `${base}-${duplicate}`);
  }
  cache.set(path, anchors);
  return anchors;
}

function validTimestamp(value) {
  const match = UTC_TIMESTAMP.exec(value);
  if (!match) return false;
  const [, year, month, day, hour, minute, second] = match.map(Number);
  if (
    year < 1 ||
    month < 1 ||
    month > 12 ||
    hour > 23 ||
    minute > 59 ||
    second > 59
  ) {
    return false;
  }
  const leapYear = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
  const daysInMonth = [31, leapYear ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][
    month - 1
  ];
  return day >= 1 && day <= daysInMonth;
}

function levelOneHeadings(frontmatter) {
  const masked = maskMarkdownCode(frontmatter.body);
  const headings = [];
  for (const match of masked.matchAll(/^ {0,3}#[ \t]+(.+?)\s*$/gm)) {
    headings.push({
      line: lineAt(masked, match.index, frontmatter.bodyStartLine),
      title: match[1].replace(/\s+#+\s*$/, '').trim(),
    });
  }
  return headings;
}

function validateMetadata(document, findings) {
  const { file, frontmatter, isIndex } = document;
  if (frontmatter.errors.some((entry) => entry.code === 'frontmatter-unclosed')) return;
  const headings = levelOneHeadings(frontmatter);
  const firstBodyLine = frontmatter.body
    .split('\n')
    .findIndex((line) => line.trim() !== '');

  if (headings.length !== 1) {
    findings.push(
      issue(
        file,
        1,
        'h1-count',
        `document must contain exactly one level-one heading; found ${headings.length}`,
      ),
    );
  }
  if (
    headings.length > 0 &&
    firstBodyLine >= 0 &&
    headings[0].line !== frontmatter.bodyStartLine + firstBodyLine
  ) {
    findings.push(
      issue(
        file,
        headings[0].line,
        'h1-position',
        'level-one heading must be the first content line',
      ),
    );
  }

  if (isIndex) {
    if (frontmatter.data !== null) {
      findings.push(
        issue(file, 1, 'index-frontmatter', 'index.md is navigation, not a concept'),
      );
    }
    return;
  }
  if (frontmatter.data === null) {
    findings.push(
      issue(file, 1, 'frontmatter-required', 'concept documents require frontmatter'),
    );
    return;
  }
  const data = frontmatter.data;
  for (const key of PROFILE.requiredFields) {
    if (!Object.hasOwn(data, key)) {
      findings.push(issue(file, 1, 'field-required', `missing required field: ${key}`));
    }
  }
  for (const key of Object.keys(data)) {
    if (!PROFILE.requiredFields.includes(key)) {
      findings.push(issue(file, 1, 'field-unknown', `unknown frontmatter field: ${key}`));
    }
  }
  for (const key of [
    'type',
    'title',
    'description',
    'timestamp',
    'status',
    'implementation_status',
    'verification_status',
    'release_status',
  ]) {
    if (
      Object.hasOwn(data, key) &&
      (typeof data[key] !== 'string' || data[key].trim() === '')
    ) {
      findings.push(issue(file, 1, 'field-type', `${key} must be a non-empty string`));
    }
  }
  for (const key of ['tags', 'audience']) {
    if (
      Object.hasOwn(data, key) &&
      (!Array.isArray(data[key]) ||
        data[key].length === 0 ||
        data[key].some((entry) => typeof entry !== 'string' || entry.trim() === ''))
    ) {
      findings.push(issue(file, 1, 'field-type', `${key} must be a non-empty string array`));
    }
    if (
      Array.isArray(data[key]) &&
      data[key].some(
        (entry) =>
          typeof entry === 'string' &&
          !/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(entry),
      )
    ) {
      findings.push(
        issue(file, 1, 'field-token', `${key} entries must be lowercase hyphenated tokens`),
      );
    }
    if (Array.isArray(data[key]) && new Set(data[key]).size !== data[key].length) {
      findings.push(issue(file, 1, 'field-duplicate', `${key} entries must be unique`));
    }
  }
  if (typeof data.type === 'string' && !PROFILE.types.includes(data.type)) {
    findings.push(
      issue(
        file,
        1,
        'type-vocabulary',
        `unknown type "${data.type}"; expected one of: ${PROFILE.types.join(', ')}`,
      ),
    );
  }
  if (typeof data.status === 'string' && !PROFILE.statuses.includes(data.status)) {
    findings.push(
      issue(
        file,
        1,
        'status-vocabulary',
        `unknown status "${data.status}"; expected one of: ${PROFILE.statuses.join(', ')}`,
      ),
    );
  }
  for (const [key, vocabulary] of [
    ['implementation_status', PROFILE.implementationStatuses],
    ['verification_status', PROFILE.verificationStatuses],
    ['release_status', PROFILE.releaseStatuses],
  ]) {
    if (typeof data[key] === 'string' && !vocabulary.includes(data[key])) {
      findings.push(
        issue(
          file,
          1,
          `${key.replace('_status', '')}-status-vocabulary`,
          `unknown ${key} "${data[key]}"; expected one of: ${vocabulary.join(', ')}`,
        ),
      );
    }
  }
  if (typeof data.timestamp === 'string' && !validTimestamp(data.timestamp)) {
    findings.push(
      issue(
        file,
        1,
        'timestamp-format',
        'timestamp must be a real ISO 8601 UTC instant (YYYY-MM-DDTHH:mm:ss[.sss]Z)',
      ),
    );
  }
  if (
    typeof data.title === 'string' &&
    headings.length === 1 &&
    data.title.trim() !== headings[0].title
  ) {
    findings.push(
      issue(
        file,
        headings[0].line,
        'title-heading-mismatch',
        `frontmatter title must match the level-one heading: ${headings[0].title}`,
      ),
    );
  }
}

function validateRawHtml(document, findings) {
  const source = maskMarkdownCode(document.frontmatter.body);
  const pattern =
    /<(?:\/?[A-Za-z][A-Za-z0-9-]*(?:\s(?:[^<>"']|"[^"]*"|'[^']*')*?)?\/?|![^<>]*|\?[^<>]*\?)>/gi;
  for (const match of source.matchAll(pattern)) {
    findings.push(
      issue(
        document.file,
        lineAt(source, match.index, document.frontmatter.bodyStartLine),
        'raw-html',
        `raw HTML is prohibited: ${match[0]}`,
      ),
    );
  }
}

function resolveLocalLink(sourcePath, destination) {
  const unescaped = destination.replace(/\\([ ()])/g, '$1');
  const hash = unescaped.indexOf('#');
  const beforeHash = hash === -1 ? unescaped : unescaped.slice(0, hash);
  const rawFragment = hash === -1 ? '' : unescaped.slice(hash + 1);
  const query = beforeHash.indexOf('?');
  const rawPath = query === -1 ? beforeHash : beforeHash.slice(0, query);
  const rawQuery = query === -1 ? '' : beforeHash.slice(query + 1);
  let pathPart;
  let fragment;
  try {
    pathPart = decodeURIComponent(rawPath);
    decodeURIComponent(rawQuery);
    fragment = decodeURIComponent(rawFragment);
  } catch {
    return { error: 'link contains invalid percent encoding' };
  }
  if (pathPart.includes('\\')) return { error: 'local links must use forward slashes' };
  if (pathPart.startsWith('/') || pathPart.startsWith('//') || isAbsolute(pathPart)) {
    return { error: 'local links must be relative' };
  }
  const target = pathPart === '' ? sourcePath : resolve(dirname(sourcePath), pathPart);
  return { target, fragment };
}

function validateLinks(
  document,
  repoRoot,
  docsRoot,
  anchorsCache,
  graph,
  invalidDocuments,
  findings,
) {
  const parsed = markdownLinks(document.frontmatter.body, document.frontmatter.bodyStartLine);
  for (const missing of parsed.missingReferences) {
    findings.push(
      issue(
        document.file,
        missing.line,
        'reference-missing',
        `undefined link reference: ${missing.label}`,
      ),
    );
  }
  for (const link of parsed.links) {
    const destination = link.destination.trim();
    if (destination === '') continue;
    if (URI_SCHEME.test(destination)) {
      let external;
      try {
        external = new URL(destination);
      } catch {
        external = null;
      }
      if (
        external === null ||
        !/^https:\/\//i.test(destination) ||
        external.protocol !== 'https:' ||
        external.hostname === '' ||
        external.username !== '' ||
        external.password !== ''
      ) {
        findings.push(
          issue(
            document.file,
            link.line,
            'link-external',
            `external links must be valid credential-free https URLs: ${destination}`,
          ),
        );
      }
      continue;
    }
    const local = resolveLocalLink(document.path, destination);
    if (local.error) {
      findings.push(
        issue(document.file, link.line, 'link-format', `${local.error}: ${destination}`),
      );
      continue;
    }
    const repoRelative = relative(repoRoot, local.target);
    if (repoRelative === '..' || repoRelative.startsWith(`..${sep}`)) {
      findings.push(
        issue(
          document.file,
          link.line,
          'link-outside-repo',
          `local target leaves the repository: ${destination}`,
        ),
      );
      continue;
    }
    if (!existsSync(local.target)) {
      findings.push(
        issue(
          document.file,
          link.line,
          'link-broken',
          `local target does not exist: ${destination}`,
        ),
      );
      continue;
    }
    if (
      lstatSync(local.target).isSymbolicLink() ||
      hasSymlinkComponent(repoRoot, local.target)
    ) {
      findings.push(
        issue(
          document.file,
          link.line,
          'link-symlink',
          `local target path must not contain symbolic links: ${destination}`,
        ),
      );
      continue;
    }
    const realTarget = realpathSync(local.target);
    const realRelative = relative(realpathSync(repoRoot), realTarget);
    if (realRelative === '..' || realRelative.startsWith(`..${sep}`)) {
      findings.push(
        issue(
          document.file,
          link.line,
          'link-outside-repo',
          `local target resolves outside the repository: ${destination}`,
        ),
      );
      continue;
    }
    if (
      local.fragment &&
      extname(local.target).toLowerCase() === '.md' &&
      !invalidDocuments.has(resolve(local.target))
    ) {
      const expected = local.fragment.toLocaleLowerCase('en-US');
      if (!markdownAnchors(local.target, anchorsCache).has(expected)) {
        findings.push(
          issue(
            document.file,
            link.line,
            'anchor-broken',
            `heading fragment does not exist: ${destination}`,
          ),
        );
      }
    }
    const targetRelative = relative(docsRoot, local.target);
    const insideDocs = targetRelative !== '..' && !targetRelative.startsWith(`..${sep}`);
    if (link.navigation && insideDocs && extname(local.target).toLowerCase() === '.md') {
      graph.get(document.path).add(resolve(local.target));
    }
  }
}

/** Validate a documentation tree and return deterministic structured findings. */
export function validateDocumentation({ repoRoot = DEFAULT_REPO_ROOT, docsDir = 'docs' } = {}) {
  const absoluteRoot = resolve(repoRoot);
  const docsRoot = resolve(absoluteRoot, docsDir);
  const findings = [];
  if (!isWithin(absoluteRoot, docsRoot)) {
    return [
      issue(
        toRepoPath(absoluteRoot, docsRoot),
        1,
        'docs-outside-repo',
        'docs directory must stay inside the repository root',
      ),
    ];
  }
  if (!existsSync(docsRoot)) {
    return [
      issue(
        toRepoPath(absoluteRoot, docsRoot),
        1,
        'docs-missing',
        'docs directory does not exist',
      ),
    ];
  }
  if (lstatSync(docsRoot).isSymbolicLink()) {
    return [
      issue(
        toRepoPath(absoluteRoot, docsRoot),
        1,
        'docs-symlink',
        'docs directory must not be a symbolic link',
      ),
    ];
  }
  if (!lstatSync(docsRoot).isDirectory()) {
    return [
      issue(
        toRepoPath(absoluteRoot, docsRoot),
        1,
        'docs-file-type',
        'docs path must be a directory',
      ),
    ];
  }
  if (!isWithin(realpathSync(absoluteRoot), realpathSync(docsRoot))) {
    return [
      issue(
        toRepoPath(absoluteRoot, docsRoot),
        1,
        'docs-outside-repo',
        'docs directory resolves outside the repository root',
      ),
    ];
  }

  const rootIndex = resolve(docsRoot, 'index.md');
  if (!existsSync(rootIndex)) {
    findings.push(
      issue(
        toRepoPath(absoluteRoot, rootIndex),
        1,
        'index-required',
        'root documentation index is missing',
      ),
    );
  } else if (
    lstatSync(rootIndex).isSymbolicLink() ||
    !lstatSync(rootIndex).isFile() ||
    hasSymlinkComponent(docsRoot, rootIndex)
  ) {
    findings.push(
      issue(
        toRepoPath(absoluteRoot, rootIndex),
        1,
        'index-file-type',
        'root documentation index must be a regular non-symlink file',
      ),
    );
  }

  const files = markdownFiles(docsRoot, absoluteRoot, findings);
  for (const path of files) {
    if (path.split(sep).at(-1).toLowerCase() === 'log.md') {
      findings.push(
        issue(
          toRepoPath(absoluteRoot, path),
          1,
          'log-prohibited',
          'log.md duplicates Git history and is prohibited by the documentation profile',
        ),
      );
    }
  }
  const documents = files
    .filter((path) => path.split(sep).at(-1).toLowerCase() !== 'log.md')
    .map((path) => {
      const file = toRepoPath(absoluteRoot, path);
      const source = readUtf8Document(path, file, findings);
      const validEncoding = source !== null;
      const frontmatter = validEncoding
        ? parseFrontmatter(source, file)
        : { data: null, body: '', bodyStartLine: 1, errors: [] };
      findings.push(...frontmatter.errors);
      return {
        path,
        file,
        frontmatter,
        validEncoding,
        isIndex: path.split(sep).at(-1).toLowerCase() === 'index.md',
      };
    });

  for (const document of documents) {
    if (!document.validEncoding) continue;
    validateMetadata(document, findings);
    validateRawHtml(document, findings);
  }

  const titles = new Map();
  for (const document of documents) {
    if (document.isIndex || typeof document.frontmatter.data?.title !== 'string') continue;
    const title = document.frontmatter.data.title
      .trim()
      .normalize('NFKC')
      .toLocaleLowerCase('en-US');
    if (title === '') continue;
    if (titles.has(title)) {
      findings.push(
        issue(
          document.file,
          1,
          'title-duplicate',
          `title duplicates ${titles.get(title)}: ${document.frontmatter.data.title}`,
        ),
      );
    } else {
      titles.set(title, document.file);
    }
  }

  const graph = new Map(documents.map((document) => [document.path, new Set()]));
  const anchorsCache = new Map();
  const invalidDocuments = new Set(
    documents.filter((document) => !document.validEncoding).map((document) => document.path),
  );
  for (const document of documents) {
    if (!document.validEncoding) continue;
    validateLinks(
      document,
      absoluteRoot,
      docsRoot,
      anchorsCache,
      graph,
      invalidDocuments,
      findings,
    );
  }
  const reachable = new Set();
  const rootIndexDocument = documents.find((document) => document.path === rootIndex);
  const pending = rootIndexDocument ? [rootIndex] : [];
  while (pending.length > 0) {
    const path = pending.pop();
    if (reachable.has(path)) continue;
    reachable.add(path);
    for (const target of graph.get(path) ?? []) pending.push(target);
  }
  // An undecodable reachable document may contain links we cannot discover, so
  // descendants behind it have unknown reachability. An undecodable document
  // that is itself unreachable cannot hide any path from the root, and definite
  // orphans must still be reported.
  const reachableInvalid = documents.some(
    (document) => !document.validEncoding && reachable.has(document.path),
  );
  for (const document of documents) {
    if (!reachable.has(document.path) && !reachableInvalid) {
      findings.push(
        issue(
          document.file,
          1,
          'document-orphan',
          'document is not reachable from docs/index.md',
        ),
      );
    }
  }

  return findings.sort(
    (a, b) =>
      compareText(a.file, b.file) || a.line - b.line || compareText(a.code, b.code) ||
      compareText(a.message, b.message),
  );
}

function parseArgs(argv) {
  let repoRoot = DEFAULT_REPO_ROOT;
  for (let index = 0; index < argv.length; index += 1) {
    if (argv[index] === '--root' && argv[index + 1]) {
      repoRoot = resolve(argv[index + 1]);
      index += 1;
    } else {
      throw new Error(`unknown or incomplete argument: ${argv[index]}`);
    }
  }
  return { repoRoot };
}

function main() {
  let options;
  let findings;
  try {
    options = parseArgs(process.argv.slice(2));
    findings = validateDocumentation(options);
  } catch (error) {
    console.error(`docs-check: ${error instanceof Error ? error.message : error}`);
    process.exitCode = 2;
    return;
  }
  if (findings.length > 0) {
    console.error(`docs-check: ${findings.length} finding(s)\n`);
    for (const finding of findings) {
      console.error(`  ${finding.file}:${finding.line} [${finding.code}] ${finding.message}`);
    }
    process.exitCode = 1;
    return;
  }
  console.log('docs-check: OK — profile, indexes, titles, and local links are valid.');
}

const isMain =
  process.argv[1] && realpathSync(resolve(process.argv[1])) === realpathSync(SCRIPT_PATH);
if (isMain) main();
