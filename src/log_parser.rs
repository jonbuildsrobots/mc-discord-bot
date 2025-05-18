// This parses the label and content out of a log line assuming that the line is formatted as follows:
// [__:__:__] [src] [label]: content
pub fn parse_line(line: &str) -> Result<(&str, &str), &'static str> {
    if line.len() < 13 {
        return Err("too short");
    }
    
    // ensure line is formatted as such
    // [__:__:__] [
    let line_bytes = line.as_bytes();
    if (line_bytes[0]  != b'[') ||
       (line_bytes[3]  != b':') ||
       (line_bytes[6]  != b':') ||
       (line_bytes[9]  != b']') ||
       (line_bytes[10] != b' ') ||
       (line_bytes[11] != b'[')
    {
        return Err("invalid format");
    }

    // Label starts 3 bytes after the `src` segment's ']' byte
    let label_start = line_bytes[12..].iter().position(|x| *x == b']').ok_or("invalid format, no src segment found")? + 15;

    // Ensure label_start is within the line's length 
    if line_bytes.len() <= label_start {
        return Err("error finding label start");
    }

    // Find label end
    let label_end = line_bytes[label_start..].iter().position(|x| *x == b']').ok_or("error finding label end")? + label_start;

    // Ensure label_end is within the line's length 
    if line_bytes.len() <= label_end {
        return Err("error finding label end");
    }

    // Content starts 3 bytes after the label's end
    let content_start = label_end + 3;

    // ensure content_start is within the line's length 
    if line_bytes.len() <= content_start {
        return Err("invalid content");
    }

    let label = match std::str::from_utf8(&line_bytes[label_start..label_end]) {
        Ok(v) => v,
        Err(_) => return Err("label not utf8"),
    };

    let content = match std::str::from_utf8(&line_bytes[content_start..]) {
        Ok(v) => v,
        Err(_) => return Err("content not utf8"),
    };

    return Ok((label, content));
}

#[cfg(test)]
mod tests {
    use super::parse_line;

    #[test]
    fn test_parse_line() {
        assert_eq!(parse_line("[__:__:__] [A] [TEST1]: content").unwrap(), ("TEST1", "content"));
        assert_eq!(parse_line("[__:__:__] [B] [TEST2]: A").unwrap(), ("TEST2", "A"));
        assert_eq!(parse_line("[__:__:__] [] [TEST2]: A").unwrap(), ("TEST2", "A"));
        assert_eq!(parse_line("[__:__:__] [] [TEST3]: ").unwrap_err(), "invalid content");
        assert_eq!(parse_line("[__:__:__] [] [").unwrap_err(), "error finding label start");
        assert_eq!(parse_line("[__:__:__] [] [abcdefg").unwrap_err(), "error finding label end");
        assert_eq!(parse_line("[__:__:__] ").unwrap_err(), "too short");
        assert_eq!(parse_line("A__:__:__] [] [").unwrap_err(), "invalid format");
    }
}