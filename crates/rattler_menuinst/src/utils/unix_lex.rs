pub struct UnixLex;

impl UnixLex {
    pub fn quote_args(args: &[String]) -> Vec<String> {
        args.iter().map(|a| Self::quote_string(a)).collect()
    }

    pub fn quote_string(s: &str) -> String {
        shlex::try_quote(s)
            .expect("Failed to quote string")
            .to_string()
    }
}
