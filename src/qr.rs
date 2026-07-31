use qrcode::{types::Color, QrCode};

const QUIET_ZONE: usize = 4;

pub fn render(payload: &str) -> Result<Vec<String>, qrcode::types::QrError> {
    let code = QrCode::new(payload.as_bytes())?;
    let qr_width = code.width();
    let width = qr_width + QUIET_ZONE * 2;
    let height = width.next_multiple_of(2);
    let mut lines = Vec::with_capacity(height / 2);

    for top in (0..height).step_by(2) {
        let mut line = String::with_capacity(width * 3);
        for x in 0..width {
            let upper = is_dark(&code, x, top, qr_width);
            let lower = is_dark(&code, x, top + 1, qr_width);
            line.push(match (upper, lower) {
                (false, false) => ' ',
                (true, false) => '▀',
                (false, true) => '▄',
                (true, true) => '█',
            });
        }
        lines.push(line);
    }

    Ok(lines)
}

fn is_dark(code: &QrCode, x: usize, y: usize, qr_width: usize) -> bool {
    let Some(qr_x) = x.checked_sub(QUIET_ZONE) else {
        return false;
    };
    let Some(qr_y) = y.checked_sub(QUIET_ZONE) else {
        return false;
    };
    qr_x < qr_width && qr_y < qr_width && code[(qr_x, qr_y)] == Color::Dark
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_standard_quiet_zone_and_half_height_rows() {
        let lines = render("hello").expect("QR code");

        assert_eq!(lines.len(), 15);
        assert!(lines.iter().all(|line| line.chars().count() == 29));
        assert!(lines[0].chars().all(|character| character == ' '));
        assert!(lines[1].chars().all(|character| character == ' '));
        assert!(lines.iter().any(|line| line.contains('█')));
    }
}
