fn main() {
    print!(
        "{}",
        agentype_adapter_local_process::LocalProcessAgentAdapter::new()
            .binding_key()
            .as_str()
    );
}
