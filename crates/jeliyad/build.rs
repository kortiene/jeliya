//! Build-time guard for the daemon's embedded web artifact (#176 §9).
//!
//! When (and only when) the packaged `embed-ui` feature is built, the daemon
//! embeds `crates/jeliya-ui/dist`. This guard makes accidental consumption of
//! React (`ui/dist`) output as the canonical artifact **fail closed**: the
//! `embed-ui` build fails unless the embedded folder carries the Dioxus
//! artifact marker and an `index.html` that loads the wasm module and carries
//! **no** Vite/React signature. A plain `cargo build` (no `embed-ui`) does
//! nothing here. #183 later replaces this marker with a content-addressed
//! sealed manifest and adds the runtime legacy-rejection.
//!
//! THREAT MODEL: this guard defends against ACCIDENTS — React output, a
//! stale or half-built artifact, a commented-out or inert loader — not
//! against an adversarial `index.html`. It is a string-level scanner, not an
//! HTML/JS parser; an artifact CRAFTED to satisfy it while misbehaving is
//! out of scope here, and the sealed content-addressed manifest (#183) is
//! the integrity boundary that owns that case.

use std::path::PathBuf;

fn main() {
    // Only the `embed-ui` build embeds — and therefore guards — the artifact.
    if std::env::var_os("CARGO_FEATURE_EMBED_UI").is_none() {
        return;
    }

    let manifest = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo"),
    );
    let dist = manifest.join("../jeliya-ui/dist");
    // Rebuild the guard (and re-embed) whenever the artifact changes.
    println!("cargo:rerun-if-changed={}", dist.display());
    println!(
        "cargo:rerun-if-changed={}",
        dist.join(".dioxus-artifact").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        dist.join("index.html").display()
    );
    // The expected-toolchain checks below read these two files; without
    // rerun tracking, editing the pin would not re-trigger the guard on an
    // incremental build and a now-stale artifact would stay embedded.
    println!(
        "cargo:rerun-if-changed={}",
        manifest.join("../../scripts/build-web.sh").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        manifest.join("../../Cargo.lock").display()
    );

    let marker = std::fs::read_to_string(dist.join(".dioxus-artifact")).unwrap_or_default();
    // Renderer AND crate identity: another Dioxus application's artifact —
    // built by the same pinned tools, marker and index fully well-formed —
    // must not embed as jeliya's UI.
    for required in ["renderer=dioxus-web", "crate=jeliya-ui"] {
        if !marker.lines().any(|line| line.trim() == required) {
            fail(&format!(
                "the embedded UI is not the jeliya Dioxus artifact: crates/jeliya-ui/dist is \
                 missing or its .dioxus-artifact marker does not declare {required}"
            ));
        }
    }

    // The marker records the canonical toolchain; a leftover dist built by an
    // older rustc or wasm-bindgen is a stale, noncanonical artifact the
    // marker itself carries enough information to reject. The expected
    // values come from the same single sources the canonical build uses:
    // pinned_rustc from scripts/build-web.sh, wasm-bindgen from Cargo.lock.
    let pinned_rustc = read_pinned_rustc(&manifest);
    let locked_wbg = read_locked_wasm_bindgen(&manifest);
    for (key, expected) in [
        ("rustc", pinned_rustc.as_str()),
        ("wasm_bindgen", locked_wbg.as_str()),
    ] {
        let recorded = marker.lines().find_map(|line| {
            line.trim()
                .strip_prefix(&format!("{key}="))
                .map(str::to_owned)
        });
        if recorded.as_deref() != Some(expected) {
            fail(&format!(
                "the embedded UI marker records {key}={} but the canonical pin is {key}={expected} — \
                 the artifact is stale; rerun scripts/build-web.sh",
                recorded.as_deref().unwrap_or("<missing>")
            ));
        }
    }

    let index = match std::fs::read_to_string(dist.join("index.html")) {
        Ok(text) => text,
        Err(_) => fail("the embedded UI has no index.html"),
    };
    // The HTML referencing a URL is not the same as the file being there: a
    // marked-but-incomplete directory (a referenced module or the stylesheet
    // deleted, marker and index intact) must fail the build, not ship a UI
    // that 404s on every load or renders without the canonical design
    // system. "Some non-empty .wasm/.js exists" is not enough either — a
    // stale, differently named file would stand in for the one the shell
    // actually loads — so validate the exact root-relative references
    // index.html makes (they are root-relative by the SPA-fallback contract
    // stated in index.html itself). HTML and JavaScript comments are
    // stripped first: a commented-out module script — or commented-out
    // import/init statements inside an active script — still carries its
    // quoted paths, and accepting them would embed a shell whose init never
    // runs.
    let index = strip_js_comments(&strip_html_comments(&index));
    let collect_refs = |text: &str, exts: &[&str]| -> Vec<String> {
        let mut refs: Vec<String> = text
            .split(['"', '\''])
            .filter(|t| t.starts_with('/') && exts.iter().any(|ext| t.ends_with(ext)))
            .map(str::to_owned)
            .collect();
        refs.sort_unstable();
        refs.dedup();
        refs
    };
    // Module references count only inside an ACTIVE module script: a path in
    // a `type="text/plain"` script, an ordinary attribute, or prose is not
    // executable, and accepting it would embed a shell whose init never runs.
    let scripts = module_script_bodies(&index);
    // A path must PARTICIPATE in executable code, not merely sit in a string
    // literal: the .js glue as an import target (`import … from '<path>'`),
    // the .wasm module in value/argument position (`init({ module_or_path:
    // '<path>' })` or a direct call argument). Whitespace is NORMALIZED to
    // single spaces (never removed — full compaction fuses `init from` into
    // `initfrom` and destroys the very boundaries being checked).
    let normalized_scripts = scripts.split_whitespace().collect::<Vec<_>>().join(" ");
    let all_refs = collect_refs(&scripts, &[".wasm", ".js"]);
    // The .js glue must be an actual import target...
    let js_refs: Vec<String> = all_refs
        .iter()
        .filter(|r| r.ends_with(".js"))
        .filter(|r| {
            ['\'', '"'].iter().any(|quote| {
                executable_position(&normalized_scripts, &format!("{quote}{r}{quote}"), true)
            })
        })
        .cloned()
        .collect();
    // ...and the wasm path must appear inside a CALL of the initializer
    // imported from ITS OWN glue — wasm-bindgen pairs `<stem>_bg.wasm` with
    // `<stem>.js` by construction, so the accepted initializer for a given
    // wasm is exactly the default import of the matching-stem glue. An
    // unrelated import (even of another real dist .js) called with the wasm
    // path initializes this module no more than an unused object literal
    // does.
    let imports = imported_default_idents(&normalized_scripts);
    let wasm_refs: Vec<String> = all_refs
        .iter()
        .filter(|r| r.ends_with(".wasm"))
        .filter(|r| {
            let file = r.rsplit('/').next().unwrap_or(r);
            let Some(stem) = file.strip_suffix("_bg.wasm") else {
                return false;
            };
            let glue_file = format!("{stem}.js");
            let glue_idents: Vec<String> = imports
                .iter()
                .filter(|(_, source)| {
                    js_refs.iter().any(|jr| jr == source)
                        && source.rsplit('/').next() == Some(glue_file.as_str())
                })
                .map(|(ident, _)| ident.clone())
                .collect();
            ['\'', '"'].iter().any(|quote| {
                inside_initializer_call(
                    &normalized_scripts,
                    &format!("{quote}{r}{quote}"),
                    &glue_idents,
                )
            })
        })
        .cloned()
        .collect();
    let mut module_refs: Vec<String> = [js_refs, wasm_refs].concat();
    if !module_refs.iter().any(|r| r.ends_with(".wasm")) {
        fail(
            "the embedded index.html has no active module script loading a root-relative .wasm \
             module in executable position — it is not the Dioxus shell",
        );
    }
    if !module_refs.iter().any(|r| r.ends_with(".js")) {
        fail(
            "the embedded index.html has no active module script importing root-relative .js \
             bindgen glue — it is not the Dioxus shell",
        );
    }
    // A stylesheet reference counts only as the HREF of an active
    // `<link rel="stylesheet">` tag: `/styles.css` in a data attribute or
    // prose applies no design system.
    let stylesheet_refs: Vec<String> = stylesheet_link_hrefs(&index)
        .into_iter()
        .filter(|href| href.starts_with('/') && href.ends_with(".css"))
        .collect();
    if stylesheet_refs.is_empty() {
        fail(
            "the embedded index.html has no active stylesheet link referencing a root-relative \
             stylesheet — the canonical shell consumes the design system as /styles.css",
        );
    }
    module_refs.extend(stylesheet_refs);
    for reference in &module_refs {
        let module = dist.join(reference.trim_start_matches('/'));
        println!("cargo:rerun-if-changed={}", module.display());
        let nonempty = std::fs::metadata(&module)
            .map(|m| m.len() > 0)
            .unwrap_or(false);
        if !nonempty {
            fail(&format!(
                "the embedded UI is missing or has an empty {reference} — a file index.html \
                 references; the artifact is incomplete"
            ));
        }
    }
    // Reject a React/Vite signature outright: a Vite dev entry, a Vite HMR
    // client, or a React source module must never be the embedded artifact.
    for signature in [
        "src/main.tsx",
        "src/main.jsx",
        "/@vite/client",
        "/@react-refresh",
        "__vite__",
    ] {
        if index.contains(signature) {
            fail(&format!(
                "the embedded index.html carries a React/Vite signature ({signature}); \
                 React output must not be embedded (#176 §9)"
            ));
        }
    }
}

/// Remove `<!-- ... -->` spans so commented-out markup cannot satisfy the
/// reference checks. An unterminated comment drops the rest of the document —
/// fail-closed: whatever hides in it does not count as a reference.
fn strip_html_comments(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start..].find("-->") {
            Some(end) => rest = &rest[start + end + 3..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Extract the bodies of `<script type="module">` elements (run after
/// comment stripping). Only these are executable module code. Case handled
/// via an ASCII-lowercased shadow (length-preserving, so indices map back);
/// an unterminated script tag or body drops the remainder — fail-closed.
fn module_script_bodies(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::new();
    let mut from = 0;
    while let Some(open_rel) = lower[from..].find("<script") {
        let open = from + open_rel;
        // Tag-NAME boundary: `<script` must be followed by whitespace or the
        // tag close, or `<scripture ...>` would count as a script and its
        // `</scripture>` would satisfy the `</script` search below.
        match lower[open + "<script".len()..].chars().next() {
            Some(c) if c.is_ascii_whitespace() || c == '>' => {}
            _ => {
                from = open + "<script".len();
                continue;
            }
        }
        let Some(tag_end_rel) = lower[open..].find('>') else {
            break;
        };
        let tag_end = open + tag_end_rel;
        // Whitespace-normalized, space-prefixed attribute matching: a bare
        // substring test would also accept `data-type="module"`, which the
        // browser does not execute as a module.
        let tag_norm = lower[open..tag_end]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let body_start = tag_end + 1;
        // Exact closing tag (optional whitespace before '>'), for the same
        // boundary reason.
        let mut close_search = body_start;
        let body_end = loop {
            let Some(rel) = lower[close_search..].find("</script") else {
                return out;
            };
            let candidate = close_search + rel;
            let after = &lower[candidate + "</script".len()..];
            if after.trim_start().starts_with('>') || after.starts_with('>') {
                break candidate;
            }
            close_search = candidate + "</script".len();
        };
        if tag_norm.contains(" type=\"module\"") || tag_norm.contains(" type='module'") {
            out.push_str(&html[body_start..body_end]);
            out.push('\n');
        }
        from = body_end;
    }
    out
}

/// Extract `<link ...>` tag texts whose attributes include
/// `rel="stylesheet"` (run after comment stripping; ASCII-lowercased shadow
/// for case). Only an active stylesheet link applies the design system.
/// Whether one quoted-path occurrence sits in EXECUTABLE position within the
/// (single-space-normalized) module script text: for the .js glue, as the
/// target of `import`/`… from` (with an identifier boundary before the
/// keyword, so `xfrom '/a.js'` does not count); for the .wasm module, in
/// value/argument position (directly after `:`, `(`, or `,`, optional space).
fn executable_position(normalized: &str, quoted: &str, is_js: bool) -> bool {
    let mut from = 0;
    while let Some(rel) = normalized[from..].find(quoted) {
        let at = from + rel;
        let before = normalized[..at].trim_end();
        let ok = if is_js {
            let keyword = ["from", "import"].into_iter().find(|k| before.ends_with(k));
            let static_pos = keyword.is_some_and(|k| {
                before[..before.len() - k.len()]
                    .chars()
                    .next_back()
                    .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'))
            });
            // A DYNAMIC import (`import('/x.js')`, space allowed before the
            // paren) loads the module just as surely — excluding it from the
            // reference set would let the guard accept an artifact missing a
            // module whose failed load leaves the packaged UI blank. The
            // boundary check mirrors the keyword form ('.' included: there
            // is no member `import()` in module syntax).
            let dynamic_pos = before
                .strip_suffix('(')
                .map(str::trim_end)
                .is_some_and(|head| {
                    head.ends_with("import")
                        && head[..head.len() - "import".len()]
                            .chars()
                            .next_back()
                            .is_none_or(|c| {
                                !(c.is_ascii_alphanumeric() || c == '_' || c == '$' || c == '.')
                            })
                });
            static_pos || dynamic_pos
        } else {
            matches!(before.chars().next_back(), Some(':' | '(' | ','))
        };
        if ok {
            return true;
        }
        from = at + 1;
    }
    false
}

/// The default-import bindings of the module script — `import NAME from
/// '<source>'` — as (identifier, source) pairs, so a caller can require that
/// the initializer it accepts was imported from a specific module.
fn imported_default_idents(normalized: &str) -> Vec<(String, String)> {
    let mut bindings = Vec::new();
    let mut from_idx = 0;
    while let Some(rel) = normalized[from_idx..].find("import ") {
        let key_at = from_idx + rel;
        // Identifier boundary: `ximport init from …` is not an import
        // statement (and not valid JS) — a preceding identifier character
        // disqualifies the match.
        let bounded = normalized[..key_at]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'));
        if !bounded {
            from_idx = key_at + "import ".len();
            continue;
        }
        let at = from_idx + rel + "import ".len();
        let rest = &normalized[at..];
        let ident: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
            .collect();
        if !ident.is_empty() {
            let after_ident = rest[ident.len()..].trim_start();
            if let Some(after_from) = after_ident.strip_prefix("from") {
                let after_from = after_from.trim_start();
                let mut chars = after_from.chars();
                if let Some(quote) = chars.next() {
                    if quote == '"' || quote == '\'' {
                        if let Some(end) = after_from[1..].find(quote) {
                            bindings.push((ident.clone(), after_from[1..1 + end].to_owned()));
                        }
                    }
                }
            }
        }
        from_idx = at;
    }
    bindings
}

/// Whether the quoted wasm path occurs inside a call of one of the imported
/// initializers: after `NAME(` (identifier boundary before NAME) and before
/// the next `)`. A guard heuristic — the canonical shell's
/// `init({ module_or_path: '<wasm>' })` shape — not a JS parser.
fn inside_initializer_call(normalized: &str, quoted: &str, idents: &[String]) -> bool {
    for ident in idents {
        let call = format!("{ident}(");
        let mut from = 0;
        while let Some(rel) = normalized[from..].find(&call) {
            let at = from + rel;
            // `.` disqualifies too: `other.init(...)` calls a PROPERTY named
            // like the import, not the imported initializer binding — the
            // real bindgen init is never invoked and the shell mounts blank.
            let boundary = normalized[..at]
                .chars()
                .next_back()
                .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_' || c == '$' || c == '.'));
            if boundary {
                let after = &normalized[at + call.len()..];
                let span = after.find(')').map_or(after, |end| &after[..end]);
                if span.contains(quoted) {
                    return true;
                }
            }
            from = at + call.len();
        }
    }
    false
}

fn stylesheet_link_hrefs(html: &str) -> Vec<String> {
    // Whitespace is NORMALIZED (single spaces), never removed: full
    // compaction destroys attribute boundaries, and `href="` would match
    // inside `data-href="`. With single-space separation the ` href=` prefix
    // carries its boundary.
    stylesheet_link_tags(html)
        .lines()
        .filter_map(|tag| {
            let normalized = tag.split_whitespace().collect::<Vec<_>>().join(" ");
            for quote in ['"', '\''] {
                let prefix = format!(" href={quote}");
                if let Some(start) = normalized.find(&prefix) {
                    let rest = &normalized[start + prefix.len()..];
                    if let Some(end) = rest.find(quote) {
                        return Some(rest[..end].to_owned());
                    }
                }
            }
            None
        })
        .collect()
}

fn stylesheet_link_tags(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::new();
    let mut from = 0;
    while let Some(open_rel) = lower[from..].find("<link") {
        let open = from + open_rel;
        // Same tag-name boundary discipline as the script scan.
        match lower[open + "<link".len()..].chars().next() {
            Some(c) if c.is_ascii_whitespace() || c == '>' => {}
            _ => {
                from = open + "<link".len();
                continue;
            }
        }
        let Some(end_rel) = lower[open..].find('>') else {
            break;
        };
        let end = open + end_rel;
        // Same attribute-boundary discipline as the module-script scan: a
        // `data-rel="stylesheet"` must not count.
        let tag_norm = lower[open..end]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if tag_norm.contains(" rel=\"stylesheet\"") || tag_norm.contains(" rel='stylesheet'") {
            out.push_str(&html[open..end]);
            out.push('\n');
        }
        from = end + 1;
    }
    out
}

/// Remove `/* ... */` spans and `//`-to-end-of-line comments so commented-out
/// import/init statements cannot satisfy the reference checks. A `//` directly
/// preceded by `:` is kept (a URL scheme separator, `https://…`, not a
/// comment) — a guard heuristic, not a JS parser; the canonical shell keeps
/// no protocol-relative references. An unterminated block comment drops the
/// remainder — fail-closed.
fn strip_js_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            match text[i + 2..].find("*/") {
                Some(end) => i += 2 + end + 2,
                None => return out,
            }
        } else if bytes[i] == b'/'
            && i + 1 < bytes.len()
            && bytes[i + 1] == b'/'
            && (i == 0 || bytes[i - 1] != b':')
        {
            match text[i..].find('\n') {
                Some(end) => i += end,
                None => return out,
            }
        } else {
            out.push(text[i..].chars().next().expect("in-bounds char"));
            i += text[i..].chars().next().expect("in-bounds char").len_utf8();
        }
    }
    out
}

/// The canonical compiler pin, read from its single source of truth in
/// scripts/build-web.sh (`pinned_rustc="X"`), so this guard and the build
/// recipe cannot drift apart.
fn read_pinned_rustc(manifest: &std::path::Path) -> String {
    let script = manifest.join("../../scripts/build-web.sh");
    let text = std::fs::read_to_string(&script)
        .unwrap_or_else(|_| fail("cannot read scripts/build-web.sh for the rustc pin"));
    text.lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("pinned_rustc=\"")
                .and_then(|rest| rest.strip_suffix('"'))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| fail("scripts/build-web.sh no longer declares pinned_rustc"))
}

/// The locked wasm-bindgen version from the workspace Cargo.lock — the same
/// derivation build-web.sh performs.
fn read_locked_wasm_bindgen(manifest: &std::path::Path) -> String {
    let lock = manifest.join("../../Cargo.lock");
    let text = std::fs::read_to_string(&lock)
        .unwrap_or_else(|_| fail("cannot read Cargo.lock for the wasm-bindgen pin"));
    let mut found = false;
    for line in text.lines() {
        let line = line.trim();
        if line == "name = \"wasm-bindgen\"" {
            found = true;
        } else if found {
            if let Some(version) = line
                .strip_prefix("version = \"")
                .and_then(|rest| rest.strip_suffix('"'))
            {
                return version.to_owned();
            }
            found = false;
        }
    }
    fail("Cargo.lock does not lock wasm-bindgen")
}

fn fail(message: &str) -> ! {
    eprintln!("embed-ui: {message}.");
    eprintln!("embed-ui: run `scripts/build-web.sh` to produce the Dioxus artifact first.");
    std::process::exit(1);
}
