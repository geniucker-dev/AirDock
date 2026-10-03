use std::io::{self, BufRead};
fn main() -> anyhow::Result<()> {
    for line in io::stdin().lock().lines() {
        let line = line?;
        let (message, key) = line
            .split_once(' ')
            .ok_or_else(|| anyhow::anyhow!("Expected message and key hex"))?;
        println!(
            "{}",
            hex::encode(airplay_windows::crypto::stream_key(
                &hex::decode(message)?,
                &hex::decode(key)?
            )?)
        );
    }
    Ok(())
}
