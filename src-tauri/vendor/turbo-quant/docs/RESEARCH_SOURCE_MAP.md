# Research source map for P24

Use these as context, not as unverified crate claims.

## TurboQuant

- Google Research blog: TurboQuant is presented as a family of quantization algorithms for LLM compression and vector search.
- arXiv 2504.19874: describes online vector quantization, TurboQuant_mse, TurboQuant_prod, random rotation, scalar quantization, and QJL residual correction.

## Current systems direction

- vLLM TurboQuant documentation describes Hadamard rotation followed by Lloyd-Max scalar quantization for keys and uniform quantization for values.
- vLLM issue discussions and other implementations show active interest in bitpacking and KV-cache compression.

## FibQuant

- arXiv 2605.11478: FibQuant targets random-access KV-cache compression with radial/angular codebooks, Beta-quantile radii, Fibonacci/Roberts-Kronecker directions, and Lloyd-Max refinement.
- FibQuant must remain a separate crate. P24 only aligns shared profiles/receipts/eval semantics.

## Engineering interpretation

TurboQuant and FibQuant should become sibling algorithm crates beneath a future `quant-governor`, not one monolithic crate.
