use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
fn main() {
    let data = std::fs::read("/Users/fboesiger/Library/Fonts/ftl____.ttf").unwrap();
    let font = FontRef::try_from_slice(&data).unwrap();

    // Check unscaled metrics to deduce units_per_em
    let ascent = font.ascent_unscaled();
    let descent = font.descent_unscaled();
    let line_gap = font.line_gap_unscaled();
    println!("ftl____.ttf (Frutiger 45 Light):");
    println!(
        "  ascent_unscaled={:.1}, descent_unscaled={:.1}, line_gap_unscaled={:.1}",
        ascent, descent, line_gap
    );
    // fontTools says: ascent=1561, descent=-430 (OS/2 typo), units_per_em=2048
    // If ab_glyph reads from hhea: hhea.ascent might differ
    println!("  Expected from OS/2: ascent=1561, descent=-430, units_per_em=2048");

    // Compute what ab_glyph thinks units_per_em is
    // h_advance_scaled = advance_units * px_scale / units_per_em
    // At px_scale=units_per_em, h_advance_scaled should equal advance_units
    let big_scale = PxScale::from(2048.0);
    let sf_big = font.as_scaled(big_scale);
    let d_glyph = font.glyph_id('D');
    let d_advance_units = sf_big.h_advance(d_glyph);
    println!(
        "  D advance at scale=2048: {:.1} (should be 1366 if units_per_em=2048)",
        d_advance_units
    );

    // Try another scale to compute the actual ratio
    let scale8 = PxScale::from(8.0);
    let sf8 = font.as_scaled(scale8);
    let d8 = sf8.h_advance(d_glyph);
    println!(
        "  D advance at scale=8: {:.3} (expected 5.336 if units_per_em=2048)",
        d8
    );

    // Back-compute units_per_em: d8 = advance_units * 8 / upm
    // advance_units = d8 * upm / 8, and also d_advance_units = advance_units * 2048 / upm
    // so: d_advance_units = (d8 * upm / 8) * 2048 / upm = d8 * 2048 / 8 = d8 * 256
    println!(
        "  Back-computed advance_units from 8pt: d8*256 = {:.1}",
        d8 * 256.0
    );
    println!(
        "  Back-computed advance_units from 2048pt: {:.1}",
        d_advance_units
    );

    // Test: what if actual upm is different?
    // d8 = advance_units * 8 / actual_upm
    // actual_upm = advance_units * 8 / d8
    // Using fontTools advance_units=1366:
    let actual_upm = 1366.0 * 8.0 / d8;
    println!(
        "  If advance_units=1366 (fontTools), then upm={:.1}",
        actual_upm
    );
}
