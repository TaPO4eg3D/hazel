use arrayvec::ArrayVec;

#[derive(Debug, Clone, Copy)]
pub enum NalType {
    Pps,
    Sps,
    Idr,
    Other,
}

#[derive(Debug, Clone, Copy)]
pub struct Nal<'a> {
    #[expect(dead_code)]
    pub data: &'a [u8],
    pub type_: NalType,
}

// TODO: Replace with SIMD implementation
pub fn annex_b_nals<'a>(mut buf: &'a [u8]) -> ArrayVec<Nal<'a>, 6> {
    let mut result = ArrayVec::new();

    loop {
        let Some((start, start_len)) = find_start_code(buf) else {
            return result;
        };

        let nal_start = start + start_len;
        let remaining = &buf[nal_start..];

        let nal_end = match find_start_code(remaining) {
            Some((next, _)) => next,
            None => remaining.len(),
        };

        let nal = &remaining[..nal_end];
        buf = &remaining[nal_end..];

        if nal.is_empty() {
            return result;
        }

        let type_ = match nal[0] & 0x1F {
            5 => NalType::Idr,
            7 => NalType::Sps,
            8 => NalType::Pps,
            _ => NalType::Other,
        };

        result.push(Nal { data: nal, type_ });
    }
}

#[inline]
fn find_start_code(buf: &[u8]) -> Option<(usize, usize)> {
    let mut zeros = 0usize;

    for (i, &b) in buf.iter().enumerate() {
        match b {
            0 => zeros += 1,
            1 if zeros >= 2 => {
                let len = if zeros >= 3 { 4 } else { 3 };

                return Some((i + 1 - len, len));
            }
            _ => zeros = 0,
        }
    }

    None
}
