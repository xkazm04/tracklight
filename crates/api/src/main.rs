//! LightTrack API — ingest + query REST service.
//!
//! Phase 1 will add axum routes: `POST /v1/events`, `GET /v1/events`, `GET /v1/costs`,
//! `GET /v1/limits/status`, plus project/key/limit management. For now this is a smoke test
//! that exercises the `core` wiring (loads the price book, prints the judge schema).

use lighttrack_core::PriceBook;

fn main() {
    println!("lighttrack-api v{} (scaffold)", env!("CARGO_PKG_VERSION"));

    match std::fs::read_to_string("config/pricing.json") {
        Ok(s) => match PriceBook::from_json_str(&s) {
            Ok(book) => println!("price book loaded: {} models", book.len()),
            Err(e) => eprintln!("price book parse error: {e}"),
        },
        Err(_) => println!("price book: config/pricing.json not found (run from workspace root)"),
    }

    println!(
        "judge verdict schema:\n{}",
        lighttrack_core::judge_verdict_schema()
    );
    println!("TODO(phase1): bind axum, wire SQLite Store, accept POST /v1/events");
}
