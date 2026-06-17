//! elenchus-parser — parses the English-like elenchus DSL into an AST.
//!
//! Style mirrors `vsm-parser`: zero-copy over `&str`, `nom` + `nom_locate`
//! for line/column tracking, and a human-friendly `^--- here` error display.
//! Syntax is line/keyword-oriented (not S-expressions) so small models cannot
//! trip on parentheses or indentation.
#![no_std]

extern crate alloc;
