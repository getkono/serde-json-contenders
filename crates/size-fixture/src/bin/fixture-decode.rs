//! Decode stdin with the selected backend.
fn main() {
    let out = size_fixture::round_trip(size_fixture::stdin(), false);
    let _ = std::io::Write::write_all(&mut std::io::stdout(), &out);
}
