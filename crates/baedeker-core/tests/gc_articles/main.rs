// Copyright (C) 2026 Industrial Algebra
// SPDX-License-Identifier: Apache-2.0

//! GC slice, R2 articles A1–A8 (hand-built, graded by logical-stats
//! accounting and structured refusal).
//!
//! Every article is a WAT program run through the full pipeline (encode →
//! decode → validate → lower → execute) and graded against the *logical*
//! accounting contract of unit 2 (`GcStats`), never process RSS.
//!
//! # Magnitudes: local vs CI
//!
//! Each article declares a FULL magnitude (10⁵–10⁷ operations) in its own
//! test. CI runs the same tests with `BAEDEKER_GC_ARTICLE_DIVISOR=<n>` set by
//! the `gc-articles` job, which divides every magnitude (`article_count`).
//! The FORMULAS are exact at both magnitudes — they are expressed in terms of
//! the actual run count, so a smaller count changes nothing but the runtime.
//!
//! # Capped articles (structured refusal)
//!
//! A1 and A2b need a real memory cap. The test re-executes *this* test binary
//! as a child process under `ulimit -v` (Linux) with `GC_ARTICLE_CHILD` set;
//! the child runs the pressure workload, catches the refusal as a clean Rust
//! error (`RuntimeErrorKind::ResourceLimitExceeded`), prints machine-checkable
//! markers and exits 0. A panic (101), abort or SIGKILL (134/137) fails the
//! parent test. See `harness::spawn_capped_child` for the protocol and
//! `harness::CappedOutcome` for the graded expectations.

mod a1;
mod a2a;
mod a2b;
mod a3;
mod a4;
mod a5;
mod a6;
mod a7;
mod a8;
mod harness;
