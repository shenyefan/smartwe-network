//! 门户登录表单的密码编码，与登录页脚本里的 `md6()` 完全一致。
//!
//! 对第 `i` 个字符（按 UTF-16 码元计数）：取其低 8 位、按位反转，再异或
//! `0x35 ^ (i & 0xff)`；结果字节随即按脚本自带的 `mc()` 规则做百分号编码
//! （空格为 `+`，只有数字、字母与 `-` `.` `_` 原样保留）。
//! 这是可逆的混淆而不是加密，只防肉眼看明文。

/// 返回 `md6()` 的结果，可直接放进表单。
pub fn encrypt_password(plain: &str) -> String {
    use std::fmt::Write as _;

    let mut out = String::with_capacity(plain.len() * 3);
    for (index, unit) in plain.encode_utf16().enumerate() {
        // 脚本里 `m()` 只看低 8 位，这里的截断与之等价。
        let byte = (unit as u8).reverse_bits() ^ 0x35 ^ (index & 0xff) as u8;
        match byte {
            b' ' => out.push('+'),
            b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z' | b'-' | b'.' | b'_' => {
                out.push(char::from(byte));
            }
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reproduces_captured_pair() {
        // 抓包得到的明文/密文对：FF AA 51 3A 1D 3C 5F BE D1 B8 61 D0 A7。
        assert_eq!(
            encrypt_password("Syf040617!zwy"),
            "%FF%AAQ%3A%1D%3C_%BE%D1%B8a%D0%A7"
        );
    }

    #[test]
    fn empty_password_encodes_to_nothing() {
        assert_eq!(encrypt_password(""), "");
    }

    #[test]
    fn byte_0x20_becomes_plus() {
        // 位置 0：0xA8 反转得 0x15，异或 0x35 得 0x20（空格）。
        assert_eq!(encrypt_password("\u{a8}"), "+");
    }

    #[test]
    fn same_character_differs_by_position() {
        assert_ne!(&encrypt_password("aa")[..1], &encrypt_password("aa")[1..]);
    }
}
