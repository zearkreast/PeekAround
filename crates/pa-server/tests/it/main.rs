#![allow(clippy::type_complexity)]
//! Integration tests (mirror packages/server/test/*.test.ts), one binary to save disk.

mod common;

mod albums_library;
mod api;
mod devices;
mod lan_android;
mod migration;
mod phone;
mod security;
mod shutdown;
mod thumbs;
mod tiles;

/// timing slack for debug builds / slow CI runners (PEEK_TEST_SLACK, default 4 in debug)
pub fn slack() -> u128 {
    std::env::var("PEEK_TEST_SLACK")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(if cfg!(debug_assertions) { 4 } else { 1 })
}
mod explore_insights;
mod grouping;
mod jobs;
mod modes;
mod skeleton_bench;
