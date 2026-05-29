# Validation Scripts

Copy these scripts into `Gloss/validation/` after reviewing them. They are conservative static gates designed to catch the exact failure classes found in the hostile audits.

Run:

```bash
bash validation/run_closing_gates.sh .
```

They do not replace Rust/frontend tests or live Ollama smoke. They are additional guardrails.
