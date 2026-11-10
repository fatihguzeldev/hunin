use std::io::{Read,Write};
#[inline(never)]
fn render_block(mut n:u64)->u64{
    for i in 0..1_000_000 {n=n.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407)^i;}
    n
}
fn main(){
    println!("ready");std::io::stdout().flush().unwrap();let mut gate=[0];std::io::stdin().read_exact(&mut gate).unwrap();if gate[0]!=b'g'{std::process::exit(2);}
    let start=std::time::Instant::now();let mut n=1;
    while start.elapsed()<std::time::Duration::from_secs(3){n=std::hint::black_box(render_block(n));}
    println!("{n}");
}
