# Release Compliance Checklist

Complete this checklist for every public release. It is an engineering release
gate, not legal advice. Items requiring facts outside the repository must be
confirmed by the release owner or qualified counsel.

## Source provenance

- [x] All code contributors had the right to submit their changes.
- [x] The release owner confirms that `tns-thin` was developed only from
  lawfully available documentation, observed interoperability behavior, and
  the public permissively licensed sources recorded in
  `crates/tns-thin/PROVENANCE.md`.
- [x] No Oracle proprietary binary was decompiled or disassembled, and no
  confidential, NDA-restricted, support-portal-only, or unlawfully obtained
  material was used.
- [x] Every newly referenced upstream revision and source area is recorded in
  `crates/tns-thin/PROVENANCE.md` and the affected source header.

## Licenses and release contents

- [x] `crates/tns-thin/Cargo.toml` declares `Apache-2.0`.
- [x] `cargo package --manifest-path crates/tns-thin/Cargo.toml --locked --list`
  includes `LICENSE-APACHE`, `LICENSE-MIT`, `NOTICE`,
  `THIRD_PARTY_NOTICES.md`, and `PROVENANCE.md`.
- [x] `cargo about generate about.hbs` reproduces
  `THIRD_PARTY_DEPENDENCIES.md` without a diff.
- [x] Every release archive includes `DISCLAIMER.md`, `RELEASE_COMPLIANCE.md`,
  the project licenses/notices, the `tns-thin` notice and provenance record,
  upstream notice files, dependency license texts, and the Rust toolchain
  copyright file.
- [x] The release binary does not bundle or dynamically require Oracle Instant
  Client unless a separately reviewed distribution plan explicitly permits it.

## Trademarks and claims

- [x] Product names, archive names, icons, and marketing do not use Oracle
  logos or imply Oracle affiliation, sponsorship, or endorsement.
- [x] Oracle marks are used only descriptively and the non-affiliation and
  trademark statements remain in the README and notices.
- [x] A release owner or counsel has reviewed the descriptive use of `TNS` in
  the crate name for each intended distribution channel.

## Encryption and export controls

`tns-thin` contains cryptographic functionality used for database
authentication and protocol interoperability. Repository checks cannot decide
the distributor's jurisdiction, destination countries, users, or filing
obligations.

- [x] The distributor has classified the release under every applicable export
  control regime and completed any required notification, filing, screening,
  or recordkeeping before publication.
- [x] Sanctioned destinations, restricted parties, and prohibited end uses have
  been addressed for the intended distribution method.

## Approval

- Release tag: v0.1.6719
- Release owner: letspurify-ux
- Review date: 2026-07-12
- Counsel/export reviewer, if required:
- Notes or filing references:
