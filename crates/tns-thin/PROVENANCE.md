# tns-thin Provenance Record

This document records the public, permissively licensed source revisions used
as references while implementing `tns-thin`. It supplements
`THIRD_PARTY_NOTICES.md`; it does not replace any license or notice.

## Reference snapshots

### python-oracledb

- Repository: https://github.com/oracle/python-oracledb
- Revision: `a7b40f112949875a2bb1449ffcb068953cd88999`
- Revision page: https://github.com/oracle/python-oracledb/commit/a7b40f112949875a2bb1449ffcb068953cd88999
- License choice for modified portions: Apache License, Version 2.0
- Snapshot detail: `src/oracledb/version.py` reports version `4.0.1`, but the
  commit SHA above is the authoritative identifier.

Reference scope:

- `src/connect.rs`: thin connection, packet, transport, protocol, and
  capability handling.
- `src/exec.rs`: thin protocol constants, data types, metadata, binds, and
  statement behavior.
- `src/pool.rs`: thin connection-pool behavior.
- `src/session.rs`: thin authentication, message encoding/decoding, execution,
  fetch, LOB, transaction, timeout, cancellation, and session behavior.

The relevant upstream material is primarily under
`src/oracledb/impl/thin/`, with shared types and codecs under
`src/oracledb/impl/base/`. This is a scope record, not a claim of a one-to-one
line translation.

### go-ora

- Repository: https://github.com/sijms/go-ora
- Revision: `ef646cf075eb78b91ddb842b0f3c49cd1a3b6a88`
- Revision page: https://github.com/sijms/go-ora/commit/ef646cf075eb78b91ddb842b0f3c49cd1a3b6a88
- License: MIT

Reference scope:

- `src/connect.rs`, `src/exec.rs`, and `src/session.rs`: protocol constants and
  observable client behavior were cross-checked against the `v2/` client.
- `src/oracle_zones.rs`: the time-zone region table was derived from
  `v3/type_coder/oracle_zones.go` and converted to Rust.

## Snapshot verification

On 2026-07-12, the local ignored reference trees were compared recursively
against clean checkouts of the revisions above, excluding only each checkout's
`.git` directory. Both comparisons had zero differences.

Only upstream license and notice files are tracked under `vendor/`; the full
reference trees are intentionally ignored and are not part of source or binary
distribution. The tracked files are:

- `vendor/python-oracledb/LICENSE.txt`
- `vendor/python-oracledb/NOTICE.txt`
- `vendor/python-oracledb/THIRD_PARTY_LICENSES.txt`
- `vendor/go-ora/LICENSE`

If a future change consults another revision or upstream project, update this
record, the source-file headers, and `THIRD_PARTY_NOTICES.md` in the same
change.
