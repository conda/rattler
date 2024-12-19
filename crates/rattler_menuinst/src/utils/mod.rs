pub fn quote_args<I, S>(args: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter()
        .map(|arg| format!(r#""{}""#, arg.as_ref()))
        .collect()
}
