# Third Party Notices

Original SPACE Query code is offered under `MIT OR Apache-2.0`. The bundled
`tns-thin` component is licensed under Apache-2.0 because portions are modified
works based on Apache-2.0 material from `python-oracledb`. Other implementation
details in the TNS thin client were developed with reference to permissively
licensed upstream projects. Keep this file with source and binary
redistributions.

## python-oracledb

Portions of the TNS thin implementation are based on or derived from the thin
protocol implementation in `python-oracledb`.

Copyright (c) 2016, 2026, Oracle and/or its affiliates.

`python-oracledb` is dual licensed under the Universal Permissive License
Version 1.0 or the Apache License Version 2.0. For the portions referenced by
this project, this project elects the Apache License, Version 2.0 option,
available at:

https://www.apache.org/licenses/LICENSE-2.0

The upstream license, notice, and third-party license files are preserved in:

- `vendor/python-oracledb/LICENSE.txt`
- `vendor/python-oracledb/NOTICE.txt`
- `vendor/python-oracledb/THIRD_PARTY_LICENSES.txt`

The exact reference snapshot is upstream commit
`a7b40f112949875a2bb1449ffcb068953cd88999`. See
`crates/tns-thin/PROVENANCE.md` for the reference scope and verification record.

## go-ora

Portions of the TNS thin implementation are based on or derived from
`go-ora`, and were also checked against its behavior and protocol constants.
The upstream license file is preserved in:

- `vendor/go-ora/LICENSE`

The exact reference snapshot is upstream commit
`ef646cf075eb78b91ddb842b0f3c49cd1a3b6a88`. See
`crates/tns-thin/PROVENANCE.md` for the reference scope and verification record.

MIT License

Copyright (c) 2020 Samy Sultan

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## ODPI-C

Oracle OCI support includes ODPI-C through `oracle` 0.6.3 and `odpic-sys`
0.1.1. SPACE Query elects the Universal Permissive License, Version 1.0 option
for this component.

Copyright (c) 2016, 2024 Oracle and/or its affiliates.

The upstream license and notice are included in:

- `licenses/odpi-c/LICENSE.txt`
- `licenses/odpi-c/NOTICE.txt`

## Native Libraries Bundled by FLTK

The release binaries statically link native libraries built by `fltk-sys`.

SPACE Query is based in part on the work of the FLTK project
(https://www.fltk.org). FLTK is distributed under the GNU Library General
Public License, Version 2, with the FLTK exceptions, including its static
linking exception.

The FLTK image libraries include code from the Independent JPEG Group. The
following acknowledgment is required for executable distributions:

> This software is based in part on the work of the Independent JPEG Group.

FLTK also builds bundled libpng and zlib code. Their source licenses preserve
the upstream copyright and license notices and are available with the exact
`fltk-sys` source selected by `Cargo.lock`.

### cfltk

The statically linked FLTK support code includes `cfltk` from `fltk-sys`
1.5.23 under the MIT License.

Copyright (c) 2019 Mohammed Alyousef

The complete license is included in `licenses/cfltk/LICENSE`.

### Zstandard

MySQL support statically links Zstandard 1.5.7 through `zstd-sys`
2.0.16+zstd.1.5.7. SPACE Query elects Zstandard's BSD License option.

Copyright (c) Meta Platforms, Inc. and affiliates. All rights reserved.

The complete BSD license is included in `licenses/zstd/LICENSE`.

## MPL Source Availability

`option-ext` 0.2.0 is included under the Mozilla Public License, Version 2.0.
Its corresponding source code is available at:

https://crates.io/api/v1/crates/option-ext/0.2.0/download

The complete MPL-2.0 text is included in `THIRD_PARTY_DEPENDENCIES.md`.

## Rust Standard Library

The binaries include portions of the Rust standard library, which is generally
distributed under `MIT OR Apache-2.0` with additional third-party notices. Each
release archive includes `RUST_COPYRIGHT.html` copied from the exact Rust
toolchain used for that build.

## Rust Dependencies

Rust dependency licenses are recorded in package metadata and locked in
`Cargo.lock`. The full license texts for the dependencies linked into the
binaries are collected in `THIRD_PARTY_DEPENDENCIES.md`, generated with
`cargo about generate about.hbs -o THIRD_PARTY_DEPENDENCIES.md` (config in
`about.toml` / `about.hbs`). Regenerate this file before distributing standalone
binaries when dependencies change.

## Trademarks

Oracle, Java, MySQL, SQL*Plus, and NetSuite are trademarks or registered
trademarks of Oracle and/or its affiliates. MariaDB is a trademark of MariaDB
Corporation Ab. Other names may be trademarks of their respective owners.
These names are used only to identify the software this project connects to or
builds on. This project is independent and is not affiliated with, endorsed by,
or sponsored by Oracle, MariaDB Corporation Ab, or any other vendor.
