package com.urc.android.proxy

/**
 * Default TLS web port the URC agent listens on. Mirrors
 * `urc_common::DEFAULT_WEB_TLS_PORT` (crates/urc-common/src/lib.rs:13). Used as
 * the fallback when a pairing URL omits `port`.
 */
const val DEFAULT_WEB_TLS_PORT: Int = 15901
