// Portions of this file have been modified from, and reimplemented in Rust
// based on, the thin protocol implementation in python-oracledb
// (https://github.com/oracle/python-oracledb),
// Copyright (c) 2016, 2026, Oracle and/or its affiliates, used under the
// Apache License, Version 2.0. This is a modified work and is not the original
// python-oracledb software. See THIRD_PARTY_NOTICES.md.

use std::collections::VecDeque;
use std::mem::ManuallyDrop;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::session::{OracleThinConfig, OracleThinSession};
use crate::OracleThinError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolOptions {
    pub max_size: usize,
    pub acquire_timeout: Duration,
}

impl Default for PoolOptions {
    fn default() -> Self {
        Self {
            max_size: 4,
            acquire_timeout: Duration::from_secs(5),
        }
    }
}

pub trait PoolableConnection: Sized {
    fn connect_for_pool(config: OracleThinConfig) -> Result<Self, OracleThinError>;
    fn begin_request_for_pool(&mut self) -> Result<(), OracleThinError>;
    fn is_healthy(&mut self) -> bool;
    fn reset_before_reuse(&mut self) -> Result<(), OracleThinError>;
    fn mark_broken(&mut self);
}

impl PoolableConnection for OracleThinSession {
    fn connect_for_pool(config: OracleThinConfig) -> Result<Self, OracleThinError> {
        let mut conn = OracleThinSession::connect(config)?;
        conn.mark_pool_managed();
        Ok(conn)
    }

    fn begin_request_for_pool(&mut self) -> Result<(), OracleThinError> {
        OracleThinSession::begin_request(self)
    }

    fn is_healthy(&mut self) -> bool {
        OracleThinSession::is_healthy(self)
    }

    fn reset_before_reuse(&mut self) -> Result<(), OracleThinError> {
        OracleThinSession::reset_before_reuse(self)
    }

    fn mark_broken(&mut self) {
        OracleThinSession::mark_broken(self);
    }
}

#[derive(Debug)]
struct PoolShared<T> {
    config: OracleThinConfig,
    options: PoolOptions,
    mutex: Mutex<PoolState<T>>,
    condvar: Condvar,
}

#[derive(Debug)]
struct PoolState<T> {
    idle: VecDeque<T>,
    open_count: usize,
    closed: bool,
}

#[derive(Debug, Clone)]
pub struct OracleThinSessionPool {
    inner: Arc<PoolShared<OracleThinSession>>,
}

impl OracleThinSessionPool {
    pub fn new(config: OracleThinConfig, options: PoolOptions) -> Self {
        Self {
            inner: Arc::new(PoolShared {
                config,
                options: PoolOptions {
                    max_size: options.max_size.max(1),
                    acquire_timeout: options.acquire_timeout,
                },
                mutex: Mutex::new(PoolState {
                    idle: VecDeque::new(),
                    open_count: 0,
                    closed: false,
                }),
                condvar: Condvar::new(),
            }),
        }
    }

    pub fn acquire(&self) -> Result<PooledThinConnection<OracleThinSession>, OracleThinError> {
        let deadline = Instant::now() + self.inner.options.acquire_timeout;
        let mut guard = self
            .inner
            .mutex
            .lock()
            .map_err(|_| OracleThinError::new("Oracle thin pool lock poisoned"))?;
        loop {
            if guard.closed {
                return Err(OracleThinError::new("Oracle thin pool is closed"));
            }

            while let Some(conn) = guard.idle.pop_front() {
                drop(guard);
                let mut conn = conn;
                let healthy = conn.is_healthy();
                if healthy {
                    if conn.begin_request_for_pool().is_err() {
                        drop(conn);
                        guard =
                            self.inner.mutex.lock().map_err(|_| {
                                OracleThinError::new("Oracle thin pool lock poisoned")
                            })?;
                        guard.open_count = guard.open_count.saturating_sub(1);
                        continue;
                    }
                    return Ok(PooledThinConnection::new(Arc::clone(&self.inner), conn));
                }
                drop(conn);
                guard = self
                    .inner
                    .mutex
                    .lock()
                    .map_err(|_| OracleThinError::new("Oracle thin pool lock poisoned"))?;
                guard.open_count = guard.open_count.saturating_sub(1);
            }

            if guard.open_count < self.inner.options.max_size {
                guard.open_count += 1;
                let config = self.inner.config.clone();
                drop(guard);
                match OracleThinSession::connect_for_pool(config) {
                    Ok(mut conn) => {
                        if let Err(err) = conn.begin_request_for_pool() {
                            guard = self.inner.mutex.lock().map_err(|_| {
                                OracleThinError::new("Oracle thin pool lock poisoned")
                            })?;
                            guard.open_count = guard.open_count.saturating_sub(1);
                            self.inner.condvar.notify_one();
                            return Err(err);
                        }
                        return Ok(PooledThinConnection::new(Arc::clone(&self.inner), conn));
                    }
                    Err(err) => {
                        guard =
                            self.inner.mutex.lock().map_err(|_| {
                                OracleThinError::new("Oracle thin pool lock poisoned")
                            })?;
                        guard.open_count = guard.open_count.saturating_sub(1);
                        self.inner.condvar.notify_one();
                        return Err(err);
                    }
                }
            }

            let now = Instant::now();
            if now >= deadline {
                return Err(OracleThinError::new(
                    "timed out waiting for a pooled Oracle thin connection",
                ));
            }
            let wait_for = deadline.saturating_duration_since(now);
            let (next_guard, timeout) = self
                .inner
                .condvar
                .wait_timeout(guard, wait_for)
                .map_err(|_| OracleThinError::new("Oracle thin pool lock poisoned"))?;
            guard = next_guard;
            if timeout.timed_out() {
                return Err(OracleThinError::new(
                    "timed out waiting for a pooled Oracle thin connection",
                ));
            }
        }
    }

    pub fn close(&self) {
        let mut guard = self
            .inner
            .mutex
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.closed = true;
        guard.idle.clear();
        guard.open_count = 0;
        self.inner.condvar.notify_all();
    }
}

#[allow(dead_code)]
fn acquire_from_pool<T: PoolableConnection>(
    state: Arc<PoolShared<T>>,
) -> Result<PooledThinConnection<T>, OracleThinError> {
    let deadline = Instant::now() + state.options.acquire_timeout;
    let mut guard = state
        .mutex
        .lock()
        .map_err(|_| OracleThinError::new("Oracle thin pool lock poisoned"))?;
    loop {
        if guard.closed {
            return Err(OracleThinError::new("Oracle thin pool is closed"));
        }

        while let Some(conn) = guard.idle.pop_front() {
            drop(guard);
            let mut conn = conn;
            let healthy = conn.is_healthy();
            if healthy {
                if conn.begin_request_for_pool().is_err() {
                    drop(conn);
                    guard = state
                        .mutex
                        .lock()
                        .map_err(|_| OracleThinError::new("Oracle thin pool lock poisoned"))?;
                    guard.open_count = guard.open_count.saturating_sub(1);
                    continue;
                }
                return Ok(PooledThinConnection::new(Arc::clone(&state), conn));
            }
            drop(conn);
            guard = state
                .mutex
                .lock()
                .map_err(|_| OracleThinError::new("Oracle thin pool lock poisoned"))?;
            guard.open_count = guard.open_count.saturating_sub(1);
        }

        if guard.open_count < state.options.max_size {
            guard.open_count += 1;
            let config = state.config.clone();
            drop(guard);
            match T::connect_for_pool(config) {
                Ok(mut conn) => {
                    if let Err(err) = conn.begin_request_for_pool() {
                        guard = state
                            .mutex
                            .lock()
                            .map_err(|_| OracleThinError::new("Oracle thin pool lock poisoned"))?;
                        guard.open_count = guard.open_count.saturating_sub(1);
                        state.condvar.notify_one();
                        return Err(err);
                    }
                    return Ok(PooledThinConnection::new(Arc::clone(&state), conn));
                }
                Err(err) => {
                    guard = state
                        .mutex
                        .lock()
                        .map_err(|_| OracleThinError::new("Oracle thin pool lock poisoned"))?;
                    guard.open_count = guard.open_count.saturating_sub(1);
                    state.condvar.notify_one();
                    return Err(err);
                }
            }
        }

        let now = Instant::now();
        if now >= deadline {
            return Err(OracleThinError::new(
                "timed out waiting for a pooled Oracle thin connection",
            ));
        }
        let wait_for = deadline.saturating_duration_since(now);
        let (next_guard, timeout) = state
            .condvar
            .wait_timeout(guard, wait_for)
            .map_err(|_| OracleThinError::new("Oracle thin pool lock poisoned"))?;
        guard = next_guard;
        if timeout.timed_out() {
            return Err(OracleThinError::new(
                "timed out waiting for a pooled Oracle thin connection",
            ));
        }
    }
}

#[derive(Debug)]
pub struct PooledThinConnection<T: PoolableConnection> {
    state: Arc<PoolShared<T>>,
    conn: ManuallyDrop<T>,
    returned: bool,
    discard_on_drop: bool,
}

impl<T: PoolableConnection> PooledThinConnection<T> {
    fn new(state: Arc<PoolShared<T>>, conn: T) -> Self {
        Self {
            state,
            conn: ManuallyDrop::new(conn),
            returned: false,
            discard_on_drop: false,
        }
    }
}

impl<T: PoolableConnection> Drop for PooledThinConnection<T> {
    fn drop(&mut self) {
        if self.returned {
            return;
        }
        self.returned = true;
        let mut conn = unsafe { ManuallyDrop::take(&mut self.conn) };
        let healthy = conn.is_healthy();
        if !self.discard_on_drop && healthy && conn.reset_before_reuse().is_ok() {
            let mut guard = self
                .state
                .mutex
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !guard.closed {
                guard.idle.push_back(conn);
                self.state.condvar.notify_one();
                return;
            }
        }
        drop(conn);
        let mut guard = self
            .state
            .mutex
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.open_count = guard.open_count.saturating_sub(1);
        self.state.condvar.notify_one();
    }
}

impl<T: PoolableConnection> PooledThinConnection<T> {
    pub fn discard(mut self) {
        self.discard_on_drop = true;
    }

    pub fn mark_broken(&mut self) {
        self.conn.mark_broken();
    }
}

impl<T: PoolableConnection> Deref for PooledThinConnection<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.conn
    }
}

impl<T: PoolableConnection> DerefMut for PooledThinConnection<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.conn
    }
}
