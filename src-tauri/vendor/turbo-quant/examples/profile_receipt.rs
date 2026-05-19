use turbo_quant::TurboQuantizer;

fn main() -> turbo_quant::Result<()> {
    let dim = 128;
    let quantizer = TurboQuantizer::new(dim, 8, 32, 42)?;
    let vector = (0..dim)
        .map(|index| ((index as f32) * 0.013).sin())
        .collect::<Vec<_>>();
    let (_code, receipt) =
        quantizer.encode_with_receipt(&vector, Some("example:deterministic-sine".into()))?;
    println!("{}", serde_json::to_string_pretty(&receipt).unwrap());
    Ok(())
}
