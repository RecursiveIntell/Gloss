---
name: p32r3-phase-gate
description: Use at every P32R3 phase boundary to emit receipts and block false completion.
---

At the end of each phase, run `python3 scripts/p32r3_phase_gate.py --run-id P32R3 --phase <phase_id> --status pass|fail`. If status is fail, write a blocker report and stop. Never declare a phase complete from prose alone.
