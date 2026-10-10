use base64::Engine as _;

const CHUNK: usize = 4096;

/// Transmits RGBA pixels as image `id` and creates a virtual placement of
/// `cols`×`rows` cells for Unicode placeholders.
pub fn transmit(id: u32, rgba: &[u8], width: u32, height: u32, cols: u16, rows: u16) -> Vec<u8> {
    let data = base64::engine::general_purpose::STANDARD.encode(rgba);
    let chunks: Vec<&[u8]> = if data.is_empty() {
        vec![&[][..]]
    } else {
        data.as_bytes().chunks(CHUNK).collect()
    };
    let mut out = Vec::with_capacity(data.len() + chunks.len() * 16 + 64);
    for (i, chunk) in chunks.iter().enumerate() {
        let more = u8::from(i + 1 < chunks.len());
        out.extend_from_slice(b"\x1b_G");
        if i == 0 {
            out.extend_from_slice(
                format!("a=T,U=1,q=2,f=32,i={id},s={width},v={height},c={cols},r={rows},")
                    .as_bytes(),
            );
        }
        out.extend_from_slice(format!("m={more};").as_bytes());
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\x1b\\");
    }
    out
}

/// Deletes image `id` and its placements.
pub fn delete(id: u32) -> Vec<u8> {
    format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\").into_bytes()
}

/// Wraps an escape sequence for tmux passthrough.
pub fn wrap_tmux(seq: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(seq.len() + 16);
    out.extend_from_slice(b"\x1bPtmux;");
    for &byte in seq {
        if byte == 0x1b {
            out.push(0x1b);
        }
        out.push(byte);
    }
    out.extend_from_slice(b"\x1b\\");
    out
}

#[cfg(test)]
mod test {
    use super::*;

    fn chunks(seq: &[u8]) -> Vec<String> {
        String::from_utf8(seq.to_vec())
            .unwrap()
            .split("\x1b\\")
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn transmit_single_chunk() {
        let seq = transmit(7, &[1, 2, 3, 4], 1, 1, 2, 3);
        assert_eq!(
            String::from_utf8(seq).unwrap(),
            "\x1b_Ga=T,U=1,q=2,f=32,i=7,s=1,v=1,c=2,r=3,m=0;AQIDBA==\x1b\\"
        );
    }

    #[test]
    fn transmit_chunks() {
        let rgba = vec![0u8; 3 * 4096]; // 16384 base64 bytes -> 4 chunks
        let parts = chunks(&transmit(9, &rgba, 64, 48, 4, 3));
        assert_eq!(parts.len(), 4);
        assert!(parts[0].starts_with("\x1b_Ga=T,U=1,q=2,f=32,i=9,s=64,v=48,c=4,r=3,m=1;"));
        assert!(parts[1].starts_with("\x1b_Gm=1;"));
        assert!(parts[3].starts_with("\x1b_Gm=0;"));
        for part in &parts {
            let payload = part.split_once(';').unwrap().1;
            assert!(payload.len() <= 4096);
        }
    }

    #[test]
    fn delete_image() {
        assert_eq!(
            String::from_utf8(delete(5)).unwrap(),
            "\x1b_Ga=d,d=I,i=5,q=2\x1b\\"
        );
    }

    #[test]
    fn tmux_wrapping() {
        let wrapped = wrap_tmux(b"\x1b_Ga=d\x1b\\");
        assert_eq!(wrapped, b"\x1bPtmux;\x1b\x1b_Ga=d\x1b\x1b\\\x1b\\".to_vec());
    }
}
