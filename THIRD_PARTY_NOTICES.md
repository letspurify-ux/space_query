# Third Party Notices

This repository is licensed under `MIT OR Apache-2.0`. Some implementation
details in the TNS thin client were developed with reference to permissively
licensed upstream projects. Keep this file with source and binary
redistributions.

## python-oracledb

Portions of the TNS thin implementation are based on or derived from the thin
protocol implementation in `python-oracledb`.

Copyright (c) 2016, 2026, Oracle and/or its affiliates.

`python-oracledb` is dual licensed under the Universal Permissive License
Version 1.0 or the Apache License Version 2.0. For the portions referenced by
this project, this project elects the UPL-1.0 option. The UPL is available at:

https://oss.oracle.com/licenses/upl/

The upstream license, notice, and third-party license files are preserved in:

- `vendor/python-oracledb/LICENSE.txt`
- `vendor/python-oracledb/NOTICE.txt`
- `vendor/python-oracledb/THIRD_PARTY_LICENSES.txt`

## go-ora

The TNS thin implementation was also checked against `go-ora` behavior and
protocol constants.

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

## Rust Dependencies

Rust dependency licenses are recorded in package metadata and locked in
`Cargo.lock`. The full license texts for the dependencies linked into the
binaries are collected in `THIRD_PARTY_DEPENDENCIES.md`, generated with
`cargo about generate about.hbs -o THIRD_PARTY_DEPENDENCIES.md` (config in
`about.toml` / `about.hbs`). Regenerate this file before distributing standalone
binaries when dependencies change.

## Trademarks

Oracle, Java, MySQL, and NetSuite are registered trademarks of Oracle and/or
its affiliates. Other names may be trademarks of their respective owners.
This project is independent and is not affiliated with, endorsed by, or
sponsored by Oracle.
