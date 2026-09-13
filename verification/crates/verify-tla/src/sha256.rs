use std::{fs::File, io::Read, path::Path};

pub(crate) fn file(path: &Path) -> Result<String, String> {
    let before =
        std::fs::symlink_metadata(path).map_err(|e| format!("inspect verifier jar: {e}"))?;
    if before.file_type().is_symlink() || !before.is_file() || before.len() > 512 * 1024 * 1024 {
        return Err("verifier jar is not a bounded regular file".into());
    }
    let mut input = Vec::new();
    let file = File::open(path).map_err(|e| format!("open verifier jar: {e}"))?;
    let after = file
        .metadata()
        .map_err(|e| format!("inspect open verifier jar: {e}"))?;
    if !after.is_file() || after.len() != before.len() {
        return Err("verifier jar identity changed while opening".into());
    }
    file.take(512 * 1024 * 1024 + 1)
        .read_to_end(&mut input)
        .map_err(|e| format!("hash verifier jar: {e}"))?;
    if input.len() > 512 * 1024 * 1024 {
        return Err("verifier jar exceeds 512 MiB integrity limit".into());
    }
    Ok(bytes(&input).iter().map(|b| format!("{b:02x}")).collect())
}

fn bytes(input: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut data = input.to_vec();
    let bit_len = (data.len() as u64) * 8;
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend_from_slice(&bit_len.to_be_bytes());
    let mut h = [
        0x6a09e667u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];
    for chunk in data.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, c) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes(c.try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 32];
    for (c, v) in out.chunks_exact_mut(4).zip(h) {
        c.copy_from_slice(&v.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn known_vector() {
        assert_eq!(super::bytes(b"fixture"), hex());
    }
    fn hex() -> [u8; 32] {
        [
            0xf1, 0x6d, 0x05, 0xec, 0x6b, 0x29, 0x24, 0x8d, 0x2c, 0x61, 0xad, 0xb1, 0xe9, 0x26,
            0x3f, 0x78, 0xe4, 0xf7, 0xba, 0xce, 0x1b, 0x95, 0x50, 0x14, 0xa2, 0xd1, 0x78, 0x72,
            0xcf, 0xe4, 0x06, 0x4d,
        ]
    }
}
