# tns-thin

`tns-thin` is a Rust implementation of a thin TCP/TNS client for connecting to
Oracle Database without linking to Oracle Instant Client, OCI, or other
proprietary Oracle client libraries.

The crate is intended for applications that need a small direct client for
basic Oracle Database connectivity from Rust. It is independent software and is
not affiliated with, endorsed by, or sponsored by Oracle.

## License and notices

This crate is distributed under the Apache License, Version 2.0. The MIT
license text included in the package applies to the identified `go-ora`
material, not as an alternative license for the crate as a whole.

No Oracle proprietary client library is bundled with this crate, and consumers
do not need to download or redistribute Oracle Instant Client to use the Rust
code in this package. Some implementation details were developed with reference
to permissively licensed upstream projects. Keep `NOTICE` and
`THIRD_PARTY_NOTICES.md` with source and binary redistributions. Exact upstream
revisions and the reference scope are recorded in `PROVENANCE.md`.

Oracle, Java, MySQL, and NetSuite are registered trademarks of Oracle and/or
its affiliates. Other names may be trademarks of their respective owners.

## Current scope

The public API currently supports:

- TCP connect by host, port, and service name.
- Username/password authentication.
- SQL and PL/SQL execution.
- Described queries with typed column metadata.
- Input, output, and input/output binds.
- REF CURSOR and implicit result set fetching.
- Commit, rollback, call timeout, cancel, and a small session pool.

This is a thin protocol implementation, not an OCI wrapper. Advanced Oracle
client features may be incomplete or unsupported.

## Installation

Add the crate to your `Cargo.toml`:

```toml
[dependencies]
tns-thin = "0.1"
```

No Oracle client library path is required.

## Connecting

```rust
use tns_thin::{ConnectTarget, OracleThinConfig, OracleThinSession};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let target = ConnectTarget::service_name("127.0.0.1", 1521, "FREE");
    let config = OracleThinConfig::new(target, "system", "password");
    let mut session = OracleThinSession::connect(config)?;

    println!("server version: {:?}", session.server_version());
    session.ping()?;
    Ok(())
}
```

`ConnectTarget::service_name` builds an Easy Connect style target such as
`//127.0.0.1:1521/FREE`.

## Running a query

Use `query_described_fetch_all` when you want both rows and column metadata:

```rust
use tns_thin::{ConnectTarget, OracleThinConfig, OracleThinSession};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let target = ConnectTarget::service_name("127.0.0.1", 1521, "FREE");
    let config = OracleThinConfig::new(target, "system", "password");
    let mut session = OracleThinSession::connect(config)?;

    let result = session.query_described_fetch_all(
        "SELECT level AS n, 'R' || TO_CHAR(level) AS label \
         FROM dual CONNECT BY level <= 3",
        100,
    )?;

    for column in &result.columns {
        println!("{}: {:?}", column.name, column.column_type);
    }
    for row in result.result.rows {
        println!("{row:?}");
    }

    Ok(())
}
```

For simpler query execution without metadata, use `query` or build a
`StatementRequest`.

## Binds and statements

Bind values are positional. Use `StatementRequest::statement` for DML or PL/SQL
and push `BindValue` entries in placeholder order.

```rust
use tns_thin::exec::{BindValue, StatementRequest};
use tns_thin::{ConnectTarget, OracleThinConfig, OracleThinSession};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let target = ConnectTarget::service_name("127.0.0.1", 1521, "FREE");
    let config = OracleThinConfig::new(target, "system", "password");
    let mut session = OracleThinSession::connect(config)?;

    let mut request = StatementRequest::statement(
        "INSERT INTO demo_table (id, name) VALUES (:1, :2)",
    );
    request.binds.push(BindValue::Number("1".to_string()));
    request.binds.push(BindValue::Text("example".to_string()));

    session.execute_typed(&request, &[])?;
    session.commit()?;

    Ok(())
}
```

For output binds, use `BindValue::Out` or `BindValue::InOut` and call
`execute_out_binds` or `execute_out_binds_with_implicit`.

## REF CURSOR example

```rust
use tns_thin::exec::{BindValue, OracleColumnType, OracleValue, StatementRequest};
use tns_thin::{ConnectTarget, OracleThinConfig, OracleThinSession};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let target = ConnectTarget::service_name("127.0.0.1", 1521, "FREE");
    let config = OracleThinConfig::new(target, "system", "password");
    let mut session = OracleThinSession::connect(config)?;

    let mut request = StatementRequest::statement(
        "BEGIN OPEN :1 FOR SELECT 1 AS n FROM dual; END;",
    );
    request.binds.push(BindValue::Out {
        column_type: OracleColumnType::Cursor,
        max_len: 1,
    });

    let values = session.execute_out_binds(&request, &[])?;
    if let Some(OracleValue::Cursor(cursor)) = values.first() {
        let rows = session.fetch_ref_cursor_all(
            cursor.cursor_id,
            cursor.columns.clone(),
            100,
        )?;
        println!("{:?}", rows.result.rows);
    }

    Ok(())
}
```

## Pooling

```rust
use std::time::Duration;
use tns_thin::pool::PoolOptions;
use tns_thin::{ConnectTarget, OracleThinConfig, OracleThinSessionPool};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let target = ConnectTarget::service_name("127.0.0.1", 1521, "FREE");
    let config = OracleThinConfig::new(target, "system", "password");
    let pool = OracleThinSessionPool::new(
        config,
        PoolOptions {
            max_size: 8,
            acquire_timeout: Duration::from_secs(10),
        },
    );

    let mut session = pool.acquire()?;
    let result = session.query("SELECT 1 FROM dual", 1)?;
    println!("{:?}", result.rows);

    pool.close();
    Ok(())
}
```

## Timeouts and cancellation

```rust
use std::time::Duration;
use tns_thin::{ConnectTarget, OracleThinConfig, OracleThinSession};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let target = ConnectTarget::service_name("127.0.0.1", 1521, "FREE");
    let config = OracleThinConfig::new(target, "system", "password");
    let mut session = OracleThinSession::connect(config)?;

    session.set_call_timeout(Some(Duration::from_secs(30)))?;
    let cancel = session.cancel_handle();

    // Send this from another thread when the in-flight call should be interrupted.
    cancel.break_execution()?;
    Ok(())
}
```

## Live tests

The live integration tests require a reachable Oracle Database listener. Set
the connection environment variables, then run ignored tests explicitly:

```sh
export ORACLE_THIN_TEST_HOST=127.0.0.1
export ORACLE_THIN_TEST_PORT=1521
export ORACLE_THIN_TEST_SERVICE=FREE
export ORACLE_THIN_TEST_USERNAME=system
export ORACLE_THIN_TEST_PASSWORD=password

cargo test --test live_tns -- --ignored
```

The tests also accept the fallback names `ORACLE_TEST_HOST`,
`ORACLE_TEST_PORT`, `ORACLE_TEST_SERVICE`, `ORACLE_TEST_USERNAME`, and
`ORACLE_TEST_PASSWORD`.
