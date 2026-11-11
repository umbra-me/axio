use std::io::BufRead;

/// Calls `on_data` with the joined `data:` payload of each server-sent event.
/// Returning `Ok(false)` stops reading.
pub fn read<R: BufRead>(
    reader: R,
    mut on_data: impl FnMut(&str) -> Result<bool, String>,
) -> Result<(), String> {
    let mut data = String::new();
    for line in reader.lines() {
        let line = line.map_err(|e| format!("stream read failed: {e}"))?;
        let line = line.strip_suffix('\r').unwrap_or(&line);
        if line.is_empty() {
            if !data.is_empty() {
                let keep_going = on_data(&data)?;
                data.clear();
                if !keep_going {
                    return Ok(());
                }
            }
        } else if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
        }
    }
    if !data.is_empty() {
        on_data(&data)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_multiline_data_and_ignores_other_fields() {
        let input = "event: a\ndata: one\ndata: two\n\n: comment\ndata:three\r\n\r\n";
        let mut got = Vec::new();
        read(input.as_bytes(), |d| {
            got.push(d.to_string());
            Ok(true)
        })
        .unwrap();
        assert_eq!(got, ["one\ntwo", "three"]);
    }

    #[test]
    fn stops_when_asked() {
        let mut n = 0;
        read("data: a\n\ndata: b\n\n".as_bytes(), |_| {
            n += 1;
            Ok(false)
        })
        .unwrap();
        assert_eq!(n, 1);
    }
}
