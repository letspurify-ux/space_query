# Third Party Notices

This crate (`tns-thin`) is licensed under `MIT OR Apache-2.0`. Some
implementation details were developed with reference to, and reimplemented in
Rust from, permissively licensed upstream projects. This crate is an
independent, modified work and contains none of the original upstream source
files. Keep this file with source and binary redistributions.

The complete text of the licenses referenced below is included with this crate:
the Apache License, Version 2.0 in `LICENSE-APACHE`, and the MIT License in
`LICENSE-MIT`.

## python-oracledb

Portions of the TNS thin implementation have been modified from, and
reimplemented in Rust based on, the thin protocol implementation in
`python-oracledb` (https://github.com/oracle/python-oracledb). The reimplemented
portions are modified works; this crate does not include the original
python-oracledb source files.

`python-oracledb` is dual licensed under the Universal Permissive License
Version 1.0 or the Apache License Version 2.0. For the portions referenced by
this crate, this crate elects the **Apache License, Version 2.0** option. The
full text of the Apache License, Version 2.0 is included with this crate in
`LICENSE-APACHE` and is also available at:

https://www.apache.org/licenses/LICENSE-2.0

The following required attribution notice is reproduced from the upstream
`NOTICE.txt`:

> Copyright (c) 2016, 2026, Oracle and/or its affiliates.

The unmodified upstream license, notice, and third-party license files
(`LICENSE.txt`, `NOTICE.txt`, `THIRD_PARTY_LICENSES.txt`) are additionally
preserved in the source repository under `vendor/python-oracledb/`:

https://github.com/letspurify-ux/space_query

## go-ora

The TNS thin implementation was also checked against `go-ora` behavior and
protocol constants, and the time-zone region table was derived from its data.
These portions are modified works; this crate does not include the original
go-ora source files. The unmodified upstream license file is additionally
preserved in the source repository under `vendor/go-ora/LICENSE`:

https://github.com/letspurify-ux/space_query

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

## Trademarks

Oracle, Java, MySQL, and NetSuite are registered trademarks of Oracle and/or
its affiliates. Other names may be trademarks of their respective owners.
This crate is independent and is not affiliated with, endorsed by, or
sponsored by Oracle.
