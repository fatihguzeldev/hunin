use std::hint::black_box;

#[inline(never)]
fn hunin_rust_hotspot(rounds: u64) -> u64 {
    let mut value = 1u64;
    for item in 0..rounds {
        value = (value ^ item).wrapping_mul(6364136223846793005).wrapping_add(1);
    }
    black_box(value)
}

fn main() {
    println!("{}", hunin_rust_hotspot(250_000_000));
    if let Some(code) = std::env::args().nth(1) {
        std::process::exit(code.parse().expect("integer exit code"));
    }
}
