//! Child-only executable compiled by process isolation tests, never a shipping binary.
fn main(){
    match std::env::args().nth(1).as_deref(){
        Some("abort")=>std::process::abort(),
        Some("panic")=>panic!("deliberate importer panic"),
        Some("hang")=>loop{std::thread::park();},
        _=>std::process::exit(2),
    }
}
