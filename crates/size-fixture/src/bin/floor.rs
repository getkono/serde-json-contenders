//! The same I/O with no JSON: the baseline every fixture is weighed against.
fn main() {
    let input = size_fixture::stdin();
    let out = if size_fixture::is_echo(&input) {
        input.to_vec()
    } else {
        vec![input.len() as u8]
    };
    let _ = std::io::Write::write_all(&mut std::io::stdout(), &out);
}
