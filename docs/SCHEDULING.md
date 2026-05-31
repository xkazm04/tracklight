# Scheduled online sampling

`lt-runner schedule` periodically samples recent live events for a project, scrubs PII, and freezes a
new **dataset** — so your evaluation data keeps tracking real traffic instead of going stale. Pair the
resulting datasets with `lt-runner bench` / `calibrate` to benchmark against fresh, representative data.

It runs either as a **daemon** (its own interval loop) or as a **single cycle** (`--once`) driven by
an external scheduler (OS cron, a systemd timer, Windows Task Scheduler, Cloud Scheduler).

## How it samples

Each cycle:
1. fetches the most recent `--n` events for the project,
2. names the dataset `"<prefix>-<id8>"` after the **newest event that carries an input** (the
   "watermark"),
3. **skips** if a dataset for that watermark already exists, or if there's nothing with an input to
   sample,
4. otherwise scrubs PII (regex always; optional `--llm-scrub` pass) and freezes the dataset.

Because the name is derived from the data (not the wall clock), the cycle is **idempotent**: idle
periods cost nothing and never produce duplicate snapshots — even across separate `--once` runs. New
traffic advances the watermark, which produces the next dataset.

> The judge/scoring engine is unbudgeted; `--llm-scrub` makes one `claude -p` call per item, so it has
> a cost. Plain regex scrubbing is free. See `docs/DECISIONS.md` D9.

## Daemon mode

```bash
export LIGHTTRACK_URL=http://127.0.0.1:8787
export LIGHTTRACK_KEY=lt_...          # a project key (or set in enforced mode); dev mode needs none
lt-runner schedule --project <id> --interval 3600 --n 50
# --interval seconds between cycles · --n events per cycle · --name-prefix <p> · --llm-scrub
```

## External schedulers (use `--once`)

Each invocation runs exactly one cycle and exits; idempotency means running "too often" is harmless.

**Linux cron** — hourly:
```cron
0 * * * * LIGHTTRACK_URL=http://127.0.0.1:8787 LIGHTTRACK_KEY=lt_... \
  /usr/local/bin/lt-runner schedule --once --project myproj --n 50 >> /var/log/lighttrack-sample.log 2>&1
```

**systemd timer** — `lighttrack-sample.service` + `lighttrack-sample.timer`:
```ini
# lighttrack-sample.service
[Service]
Type=oneshot
Environment=LIGHTTRACK_URL=http://127.0.0.1:8787
Environment=LIGHTTRACK_KEY=lt_...
ExecStart=/usr/local/bin/lt-runner schedule --once --project myproj --n 50

# lighttrack-sample.timer
[Timer]
OnCalendar=hourly
Persistent=true
[Install]
WantedBy=timers.target
```

**Windows Task Scheduler** — hourly:
```powershell
$env:LIGHTTRACK_URL = "http://127.0.0.1:8787"
schtasks /Create /TN "LightTrack sample" /SC HOURLY `
  /TR "C:\path\lt-runner.exe schedule --once --project myproj --n 50"
```

**GCP Cloud Scheduler** (Phase 5) — trigger a Cloud Run **job** running the same `schedule --once`
command on a cron schedule; the runner reads `LIGHTTRACK_URL`/`LIGHTTRACK_KEY` from the environment /
Secret Manager.
