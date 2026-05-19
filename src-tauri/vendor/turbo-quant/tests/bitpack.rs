use turbo_quant::bitpack;

#[test]
fn indices_roundtrip_across_widths() {
    for bits in 1..=16 {
        let max = (1u32 << bits) - 1;
        let values = (0..37)
            .map(|index| ((index * 17) as u32 % (max + 1)) as u16)
            .collect::<Vec<_>>();
        let packed = bitpack::pack_indices(&values, bits).unwrap();
        assert_eq!(packed.len(), (values.len() * bits as usize).div_ceil(8));
        assert_eq!(
            bitpack::unpack_indices(&packed, values.len(), bits).unwrap(),
            values
        );
    }
}

#[test]
fn signs_roundtrip_and_padding_is_checked() {
    let signs = [-1, 1, 1, -1, 1, -1, -1, 1, 1];
    let mut packed = bitpack::pack_signs(&signs).unwrap();
    assert_eq!(bitpack::unpack_signs(&packed, signs.len()).unwrap(), signs);
    packed[1] |= 0b1111_1110;
    assert!(bitpack::unpack_signs(&packed, signs.len()).is_err());
}

#[test]
fn out_of_range_index_rejected() {
    assert!(bitpack::pack_indices(&[0, 8], 3).is_err());
}
