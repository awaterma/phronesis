# Notices

phronesis is MIT-licensed (see [LICENSE](LICENSE)). The seed rule packs
shipped with `phr-mcp init` are written from scratch — no code or
prose is copied from upstream sources — but several packs draw on
prior art that deserves explicit credit.

## Rust pack — `phr-mcp init --packs rust`

The Rust pack's rule logic (borrow-types, deny-warnings, string-concat,
clone-discipline, no-unwrap-in-src, and the audit-only idiom warnings)
was distilled from a longer working document, originally maintained
in a sibling project, that itself compiles guidance from several
upstream sources. We acknowledge each link in that chain:

**[rust-unofficial/patterns](https://github.com/rust-unofficial/patterns)**
— Mozilla Public License 2.0 (MPL-2.0)

The borrow-types, deny-warnings, and string-concat rules trace back
to idiom guidance in the *Rust Unofficial Patterns* book. The
companion guide
[`crates/phronesis-mcp/docs/RUST-PATTERNS-GUIDE.md`](crates/phronesis-mcp/docs/RUST-PATTERNS-GUIDE.md)
acknowledges this in its opening paragraph and reuses the upstream's
idiom / pattern / anti-pattern taxonomy. Rule messages cite the source
as "the patterns guide" inline.

Nothing from the upstream is redistributed verbatim; we cite section
names and link to the public book. MPL-2.0 is file-level copyleft and
applies to *Modifications of Covered Software* — our rule logic and
the patterns guide prose are written in our own voice, so the
copyleft clause doesn't propagate.

**John Nunley, "Rust's Block Pattern" (December 18, 2025)**

The Rust pack's two audit-phase rules `audit-rust-let-binding-count-
high` and `audit-rust-let-mut-count-high` are derived directly from
John Nunley's "Rust's Block Pattern" post. The ADRs at
`.phronesis/wiki/decisions/2026-06-04-rust-let-{mut,binding}-count-
high.md` cite the post as their canonical source. Rule warning
messages link to the post inline.

**Other web sources**

Additional idiom guidance in the working document was compiled from
miscellaneous blog posts, forum threads, and the official Rust API
Guidelines (rust-lang/api-guidelines). Where specific phrasings or
examples are recognizable, the working document attributes them
inline; we record the general debt here.

## Swift pack — `phr-mcp init --packs swift`

**[eleev/swift-design-patterns](https://github.com/eleev/swift-design-patterns)**
— MIT License

The Singleton, ValueBinding, and force-bang trio framings used in the
Swift pack rules (`audit-swift-mutable-singleton`,
`warn-swift-force-cast`) draw from this catalog of Swift design
pattern playgrounds. Rule messages cite specific sections inline (e.g.
*"§Creational/Singleton"*); the ADRs under
`.phronesis/wiki/decisions/2026-06-04-swift-*` link to the upstream
repo.

**[realm/SwiftLint](https://github.com/realm/SwiftLint)** — MIT License

The `audit-swift-legacy-constructor` and `audit-swift-legacy-random`
rules mirror SwiftLint's `legacy_constructor` and `legacy_random`
default-enabled rules. We re-implemented the token detection from
scratch using phronesis predicates (`new_content_contains` + `or`
clauses); no SwiftLint source code or rule descriptions are copied.
The ADRs link to SwiftLint's hosted rule documentation.

## Python packs — `phr-mcp init --packs python,python-patterns`

**[brandon-rhodes/python-patterns](https://github.com/brandon-rhodes/python-patterns)**
— no license declared (effectively "all rights reserved")

The `python-patterns` pack (13 design-pattern advisories: Singleton,
Flyweight, Composite, Iterator, Decorator, Composition over Inheritance,
Prebound Methods, Global Object, Sentinel Object, and related) draws on
design-pattern concepts originally discussed in the
[`python-patterns.guide`](https://python-patterns.guide/) site, authored by
Brandon Rhodes (© 2018–2020). The upstream repository declares no license,
so its content defaults to full copyright under default-copyright rules.

The 13 tree-sitter predicates in `syntax/python.rs` are written from
scratch — no code is copied. The rule messages describe each pattern and its
heuristic limit in our own voice and link to the relevant guide page for
attribution and further reading. The predicates detect syntactic shapes (e.g.
an `__new__` override with an `_instance` attribute, positive same-subject
`if`/`elif` dispatch across non-builtin domain types, and
`type(name, bases, ns)` calls) that are standard Python constructs recognized
across the ecosystem; the patterns themselves (Gang-of-Four, prebound
methods, etc.) are public knowledge documented in many sources.

The base `python` pack (11 correctness rules: bare-except, mutable default
args, swallowed exceptions, print-in-src, etc.) is sourced from Ruff's
public rule catalogue, Python documentation, and standard PEPs — all
permissively licensed or public-domain. No license concern applies to the
base pack.

If the upstream project adds an open license or the author requests
removal of derived phrasings, the rule messages should be updated or the
pack removed accordingly.

---

If you contributed prior art that you'd like to see acknowledged here
(or if any attribution above mischaracterizes your work), open an
issue or PR.
