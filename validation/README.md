# Validation Scripts

Copy these scripts into `Gloss/validation/` after reviewing them. They are conservative static gates designed to catch the exact failure classes found in the hostile audits.

Run:

```bash
bash validation/run_closing_gates.sh .
```

They do not replace Rust/frontend tests or live Ollama smoke. They are additional guardrails.

The Linux package CI job runs the actual AppImage through the current twelve-case
native workflow against pinned, isolated Ollama models:

```bash
xvfb-run -a dbus-run-session -- python3 validation/gloss_installer_smoke_gate.py \
  --repo . --build --require-integrated --receipt .codex-run-receipts/linux-package/receipt.json
```

Integrated mode requires a disposable GitHub-hosted Linux x86_64 runner and the
dependencies installed by `.github/workflows/ci.yml`, including `xdotool` for the
native folder chooser. The installer owns the fresh build and extracted `AppRun`;
the existing canary owns runtime/model setup and the existing desktop driver owns
all case observations. Evidence nests under the package receipt directory. Omitting
`--require-integrated` retains the startup/restart baseline only. Neither mode
certifies signing, other platforms, user-installed models or GPU behavior.
