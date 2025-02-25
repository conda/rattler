pub fn quote_args(args: &[String]) -> Vec<String> {
    if args.len() > 2
        && (args[0].to_uppercase().contains("CMD.EXE")
            || args[0].to_uppercase().contains("%COMSPEC%"))
        && (args[1].to_uppercase() == "/K" || args[1].to_uppercase() == "/C")
        && args[2..].iter().any(|arg| arg.contains(' '))
    {
        let cmd = ensure_pad(&args[0], '"');
        let flag = args[1].clone();
        let quoted_args = args[2..]
            .iter()
            .map(|arg| ensure_pad(arg, '"'))
            .collect::<Vec<_>>()
            .join(" ");
        vec![cmd, flag, format!("\"{}\"", quoted_args)]
    } else {
        args.iter().map(|s| quote_string(s)).collect()
    }
}

pub fn quote_string(s: &str) -> String {
    let s = s.trim_matches('"').to_string();
    if s.starts_with('-') || s.starts_with(' ') {
        s
    } else if s.contains(' ') || s.contains('/') {
        format!("\"{s}\"")
    } else {
        s
    }
}

pub fn ensure_pad(name: &str, pad: char) -> String {
    if name.is_empty() || (name.starts_with(pad) && name.ends_with(pad)) {
        name.to_string()
    } else {
        format!("{pad}{name}{pad}")
    }
}

#[cfg(test)]
mod tests {
    use super::{ensure_pad, quote_args, quote_string};

    #[test]
    fn test_quote_args() {
        let args = vec![
            "cmd.exe".to_string(),
            "/C".to_string(),
            "echo".to_string(),
            "Hello World".to_string(),
        ];
        let expected = vec![
            "\"cmd.exe\"".to_string(),
            "/C".to_string(),
            "\"\"echo\" \"Hello World\"\"".to_string(),
        ];
        assert_eq!(quote_args(&args), expected);
    }

    #[test]
    fn test_quote_string() {
        assert_eq!(quote_string("Hello World"), "\"Hello World\"");
        assert_eq!(quote_string("Hello"), "Hello");
        assert_eq!(quote_string("-Hello"), "-Hello");
        assert_eq!(quote_string(" Hello"), " Hello");
        assert_eq!(quote_string("Hello/World"), "\"Hello/World\"");
    }

    #[test]
    fn test_ensure_pad() {
        assert_eq!(ensure_pad("conda", '_'), "_conda_");
        assert_eq!(ensure_pad("_conda_", '_'), "_conda_");
        assert_eq!(ensure_pad("", '_'), "");
    }
}
