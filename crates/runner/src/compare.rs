//! Comparison mode: generate outputs from each target, judge them, compare quality × cost × latency.

use anyhow::Result;
use serde_json::json;

use lighttrack_core::{BenchTarget, Benchmark, BenchmarkCase, ModelPriceRow, Rubric};
use lighttrack_engine::{generate, parse_judge_spec, EngineConfig};

use crate::bench::judge_output;
use crate::cli::Cli;
use crate::http::{get, post};
use crate::util::{percentiles, price_gen_cost};

pub(crate) fn run_compare(
    cli: &Cli,
    http: &reqwest::blocking::Client,
    engine: &EngineConfig,
    bench: &Benchmark,
    cases: &[BenchmarkCase],
    targets: &[BenchTarget],
    samples: u32,
) -> Result<()> {
    let (jp, jm) = parse_judge_spec(&bench.judge_model);
    println!(
        "benchmark '{}' COMPARE: {} target(s) × {} case(s), judge={jp}/{jm}",
        bench.name,
        targets.len(),
        cases.len(),
    );
    let rubric: Option<Rubric> = match &bench.rubric_id {
        Some(rid) => Some(get(cli, http, &format!("/v1/rubrics/{rid}"))?),
        None => None,
    };
    // For providers whose API doesn't return a $ cost (e.g. Gemini/OpenAI), price by tokens from the DB.
    let prices: Vec<ModelPriceRow> = get(cli, http, "/v1/prices").unwrap_or_default();

    // (label, mean, pass_rate, gen_cost, judge_cost, p50_ms, errored)
    let mut rows: Vec<(String, f64, f64, f64, f64, u64, u32)> = Vec::new();
    for t in targets {
        let label = t
            .label
            .clone()
            .unwrap_or_else(|| format!("{}/{}", t.provider, t.model));
        println!("\n-- target {label} --");
        let (mut overall_sum, mut passes, mut judged, mut gen_cost, mut judge_cost, mut errored) =
            (0.0_f64, 0u32, 0u32, 0.0_f64, 0.0_f64, 0u32);
        let mut latencies: Vec<u64> = Vec::new();

        for (i, case) in cases.iter().enumerate() {
            let gen = match generate(
                engine,
                &t.provider,
                &t.model,
                t.system_prompt.as_deref(),
                &case.input,
            ) {
                Ok(g) => g,
                Err(e) => {
                    println!("  case {}: generation skipped — {e}", i + 1);
                    errored += 1;
                    continue;
                }
            };
            gen_cost += gen.cost_usd.unwrap_or_else(|| {
                price_gen_cost(&prices, &t.provider, &t.model, gen.input_tokens, gen.output_tokens)
            });
            if let Some(l) = gen.latency_ms {
                latencies.push(l);
            }
            let (score, pass, jcost) =
                judge_output(engine, &jp, &jm, &rubric, bench, case, &gen.output, samples, &prices)?;
            judge_cost += jcost;
            overall_sum += score;
            if pass {
                passes += 1;
            }
            judged += 1;
            println!("  case {}: generated → score={score:.2} pass={pass}", i + 1);
        }

        let mean = if judged > 0 { overall_sum / judged as f64 } else { 0.0 };
        let pass_rate = if judged > 0 { passes as f64 / judged as f64 } else { 0.0 };
        let (p50, p95) = percentiles(&mut latencies);
        rows.push((label.clone(), mean, pass_rate, gen_cost, judge_cost, p50.unwrap_or(0), errored));

        let report = json!({
            "mode": "compare", "target": label, "provider": t.provider, "model": t.model,
            "prompt_label": t.label, "gen_cost_usd": gen_cost, "judge_cost_usd": judge_cost,
            "errored_cases": errored,
        });
        let run = json!({
            "benchmark_id": bench.id, "n_cases": judged, "mean_score": mean, "pass_rate": pass_rate,
            "cost_usd": gen_cost + judge_cost, "status": "compared",
            "p50_latency_ms": p50, "p95_latency_ms": p95, "report": report,
        });
        post(cli, http, "/v1/benchmark-runs", &run)?;
    }

    println!("\n=== comparison ===");
    println!(
        "{:<26} {:>6} {:>7} {:>10} {:>10} {:>8} {:>4}",
        "target", "mean", "pass%", "gen$", "judge$", "p50ms", "err"
    );
    for (label, mean, pr, gc, jc, p50, err) in &rows {
        println!(
            "{label:<26} {mean:>6.2} {:>6.0}% {gc:>10.5} {jc:>10.5} {p50:>8} {err:>4}",
            pr * 100.0
        );
    }
    if let Some(best) = rows
        .iter()
        .filter(|r| r.6 < cases.len() as u32)
        .max_by(|a, b| a.1.total_cmp(&b.1))
    {
        println!("best mean: {} ({:.2})", best.0, best.1);
    }
    Ok(())
}
