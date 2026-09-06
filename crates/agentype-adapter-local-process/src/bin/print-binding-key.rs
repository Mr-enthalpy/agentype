fn main() {
    match agentype_adapter_local_process::LocalProcessAgentAdapter::try_new() {
        Ok(adapter) => print!("{}", adapter.binding_key().as_str()),
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    }
}
