#[unsafe(no_mangle)]
pub extern "C" fn cpu_hold() -> u64 {
    let mut result = 1u64;
    for n in 0..400_000_000u64 {
        result = (result ^ n).wrapping_mul(6364136223846793005).wrapping_add(1);
    }
    std::hint::black_box(result)
}
