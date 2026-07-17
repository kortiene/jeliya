import assert from 'node:assert/strict';
import {
  mkdtempSync,
  mkdirSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { afterEach, test } from 'node:test';

import { parseFrontmatter, validateDocumentation } from './check-docs.mjs';

const tempRoots = [];

afterEach(() => {
  for (const root of tempRoots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function repo(files) {
  const root = mkdtempSync(join(tmpdir(), 'jeliya-docs-check-'));
  tempRoots.push(root);
  for (const [path, contents] of Object.entries(files)) {
    const target = join(root, path);
    mkdirSync(dirname(target), { recursive: true });
    writeFileSync(target, contents);
  }
  return root;
}

function concept({
  title,
  type = 'Guide',
  status = 'canonical',
  implementationStatus = 'implemented',
  verificationStatus = 'verified',
  releaseStatus = 'not-applicable',
  timestamp = '2026-07-11T00:00:00Z',
  body = '',
}) {
  return `---
type: "${type}"
title: "${title}"
description: "Documentation for ${title}."
tags: ["docs", "testing"]
timestamp: "${timestamp}"
status: "${status}"
implementation_status: "${implementationStatus}"
verification_status: "${verificationStatus}"
release_status: "${releaseStatus}"
audience: ["contributors"]
---

# ${title}

${body}
`;
}

test('restricted frontmatter parser accepts only double-quoted strings and flow arrays', () => {
  const parsed = parseFrontmatter(`---
type: "Guide"
title: "Safe docs"
description: "Deterministic YAML subset"
tags: ["docs", "safe-deterministic"]
audience: ["contributors", "client-authors"]
---
# Safe docs
`);

  assert.deepEqual({ ...parsed.data }, {
    type: 'Guide',
    title: 'Safe docs',
    description: 'Deterministic YAML subset',
    tags: ['docs', 'safe-deterministic'],
    audience: ['contributors', 'client-authors'],
  });
  assert.equal(parsed.bodyStartLine, 8);
  assert.deepEqual(parsed.errors, []);
});

test('restricted frontmatter parser rejects executable YAML features and duplicate keys', () => {
  const parsed = parseFrontmatter(`---
type: "Guide"
type: "Reference"
title: &shared Unsafe
description: *shared
---
`);

  assert.deepEqual(
    parsed.errors.map((entry) => entry.code),
    ['frontmatter-duplicate', 'frontmatter-value', 'frontmatter-value'],
  );
});

test('restricted frontmatter parser rejects plain, single-quoted, and unquoted array strings', () => {
  const parsed = parseFrontmatter(`---
type: Guide
title: 'Unsafe style'
tags: [docs]
---
`);

  assert.deepEqual(
    parsed.errors.map((entry) => entry.code),
    ['frontmatter-value', 'frontmatter-value', 'frontmatter-value'],
  );
});

test('valid profile, nested indexes, references, and fragments pass', () => {
  const root = repo({
    'docs/index.md': `# Documentation

- [Overview](overview.md)
- [Operations](operations/index.md)
`,
    'docs/overview.md': concept({
      title: 'Overview',
      body: `See the [runbook][runbook].

[runbook]: operations/runbook.md#recovery
`,
    }),
    'docs/operations/index.md': `# Operations

- [Runbook](runbook.md)
`,
    'docs/operations/runbook.md': concept({
      title: 'Recovery runbook',
      type: 'Runbook',
      body: `## Recovery

Recover from a failed node.
`,
    }),
  });

  assert.deepEqual(validateDocumentation({ repoRoot: root }), []);
});

test('index pages need no concept frontmatter and code examples are not links', () => {
  const root = repo({
    'docs/index.md': `# Documentation

- [Guide](guide.md)

\`[not a link](missing-inline.md)\`

\`\`\`markdown
[not a link](missing-fenced.md)
\`\`\`
`,
    'docs/guide.md': concept({ title: 'Guide' }),
  });

  assert.deepEqual(validateDocumentation({ repoRoot: root }), []);
});

test('required fields, controlled vocabularies, and real UTC timestamps are enforced', () => {
  const root = repo({
    'docs/index.md': '# Documentation\n\n- [Bad](bad.md)\n',
    'docs/bad.md': `---
type: "Unknown"
title: "Bad metadata"
description: "Invalid on purpose"
tags: []
timestamp: "2026-02-30T00:00:00Z"
status: "final"
implementation_status: "complete"
verification_status: "proven"
release_status: "shipping"
---
# Bad metadata
`,
  });

  const codes = validateDocumentation({ repoRoot: root }).map((entry) => entry.code);
  assert.deepEqual(codes, [
    'field-required',
    'field-type',
    'implementation-status-vocabulary',
    'release-status-vocabulary',
    'status-vocabulary',
    'timestamp-format',
    'type-vocabulary',
    'verification-status-vocabulary',
  ]);
});

test('broken files, fragments, relative-link policy, and references are reported', () => {
  const root = repo({
    'docs/index.md': '# Documentation\n\n- [Guide](guide.md)\n',
    'docs/guide.md': concept({
      title: 'Guide',
      body: `[missing](no-such-file.md)

[bad fragment](guide.md#not-a-heading)

[absolute](/README.md)

[outside](../../outside.md)

[undefined][nowhere]
`,
    }),
  });

  const findings = validateDocumentation({ repoRoot: root });
  assert.deepEqual(
    findings.map((entry) => entry.code).sort(),
    ['anchor-broken', 'link-broken', 'link-format', 'link-outside-repo', 'reference-missing'],
  );
});

test('local links cannot traverse a symlink outside the repository', () => {
  const root = repo({
    'docs/index.md': '# Documentation\n\n- [Guide](guide.md)\n',
    'docs/guide.md': concept({
      title: 'Guide',
      body: '[outside](external/secret.md)',
    }),
  });
  const external = mkdtempSync(join(tmpdir(), 'jeliya-docs-external-'));
  tempRoots.push(external);
  writeFileSync(join(external, 'secret.md'), '# Secret\n');
  symlinkSync(external, join(root, 'docs/external'));

  assert.deepEqual(
    validateDocumentation({ repoRoot: root }).map((entry) => entry.code),
    ['link-outside-repo'],
  );
});

test('only credential-free HTTPS external links are accepted', () => {
  const root = repo({
    'docs/index.md': '# Documentation\n\n- [Guide](guide.md)\n',
    'docs/guide.md': concept({
      title: 'Guide',
      body: `[secure](https://example.com/docs)

[http](http://example.com)
[script](javascript:alert(1))
[mail](mailto:docs@example.com)
[opaque](https:example.com)
[credentials](https://user:secret@example.com)
<http://example.com/autolink>
`,
    }),
  });

  const findings = validateDocumentation({ repoRoot: root });
  assert.equal(findings.filter((entry) => entry.code === 'link-external').length, 6);
  assert.equal(findings.length, 6);
});

test('unknown fields, invalid tokens, and repeated discovery tokens are rejected', () => {
  const root = repo({
    'docs/index.md': '# Documentation\n\n- [Guide](guide.md)\n',
    'docs/guide.md': `---
type: "Guide"
title: "Guide"
description: "Invalid discovery metadata."
tags: ["Docs", "Docs"]
timestamp: "2026-07-11T00:00:00Z"
status: "canonical"
implementation_status: "implemented"
verification_status: "verified"
release_status: "not-applicable"
audience: ["client-authors", "client-authors"]
owner: "nobody"
---

# Guide
`,
  });

  assert.deepEqual(
    validateDocumentation({ repoRoot: root }).map((entry) => entry.code),
    ['field-duplicate', 'field-duplicate', 'field-token', 'field-unknown'],
  );
});

test('a concept needs one first-position H1 exactly matching its title', () => {
  const root = repo({
    'docs/index.md': '# Documentation\n\n- [Guide](guide.md)\n',
    'docs/guide.md': `---
type: "Guide"
title: "Expected title"
description: "Invalid heading contract."
tags: ["docs"]
timestamp: "2026-07-11T00:00:00Z"
status: "canonical"
implementation_status: "implemented"
verification_status: "verified"
release_status: "not-applicable"
audience: ["contributors"]
---

Intro before the heading.

# Different title
`,
  });

  assert.deepEqual(
    validateDocumentation({ repoRoot: root }).map((entry) => entry.code),
    ['h1-position', 'title-heading-mismatch'],
  );
});

test('multiple real H1 headings are rejected while fenced examples are ignored', () => {
  const root = repo({
    'docs/index.md': '# Documentation\n\n- [Guide](guide.md)\n',
    'docs/guide.md': concept({
      title: 'Guide',
      body: `# Second real heading

\`\`\`markdown
# Example heading
\`\`\`
`,
    }),
  });

  assert.deepEqual(
    validateDocumentation({ repoRoot: root }).map((entry) => entry.code),
    ['h1-count'],
  );
});

test('index frontmatter, raw HTML, and log.md are prohibited', () => {
  const root = repo({
    'docs/index.md': `---
title: "Not a concept"
---

# Documentation

- [Guide](guide.md)
`,
    'docs/guide.md': concept({
      title: 'Guide',
      body: `<!-- comments are allowed -->

<script>alert('no')</script>

\`<span>code is allowed</span>\`
`,
    }),
    'docs/log.md': '# Duplicated history\n',
  });

  assert.deepEqual(
    validateDocumentation({ repoRoot: root }).map((entry) => entry.code),
    ['raw-html', 'raw-html', 'index-frontmatter', 'log-prohibited'],
  );
});

test('duplicate titles and documents absent from every index are reported', () => {
  const root = repo({
    'docs/index.md': '# Documentation\n\n- [A](a.md)\n',
    'docs/a.md': concept({ title: 'Same title' }),
    'docs/b.md': concept({ title: 'same title' }),
  });

  const findings = validateDocumentation({ repoRoot: root });
  assert.deepEqual(
    findings.map((entry) => [entry.file, entry.code]),
    [
      ['docs/b.md', 'document-orphan'],
      ['docs/b.md', 'title-duplicate'],
    ],
  );
});

test('developer documentation matches the MSRV and complete CI job matrix', () => {
  const cargo = readFileSync(new URL('../Cargo.toml', import.meta.url), 'utf8');
  const readme = readFileSync(new URL('../README.md', import.meta.url), 'utf8');
  const contributing = readFileSync(new URL('../CONTRIBUTING.md', import.meta.url), 'utf8');
  const ci = readFileSync(new URL('../.github/workflows/ci.yml', import.meta.url), 'utf8');
  const msrv = cargo.match(/^rust-version\s*=\s*"([^"]+)"$/m)?.[1];
  assert.ok(msrv);
  const displayMsrv = /^\d+\.\d+$/.test(msrv) ? `${msrv}.0` : msrv;
  const escapedMsrv = displayMsrv.replaceAll('.', '\\.');
  assert.match(readme, new RegExp(`\\*\\*${escapedMsrv}`));
  assert.match(readme, new RegExp(`want ${escapedMsrv}\\+`));
  assert.match(ci, new RegExp(`Setup Rust ${escapedMsrv}`));
  assert.doesNotMatch(readme, /\b1\.80\b/);

  const jobs = [...ci.matchAll(/^  ([a-z][a-z0-9-]+):\n    name:/gm)]
    .map((match) => match[1]);
  assert.deepEqual(jobs, [
    'docs-ui',
    'ui-e2e',
    'flutter',
    'linux-flutter',
    'rust-runtime',
    'msrv',
    'windows-installer',
    'dependency-security',
  ]);
  for (const job of jobs) assert.match(contributing, new RegExp('`' + job + '`'));
  assert.match(contributing, /manually without publishing a release/);
});
