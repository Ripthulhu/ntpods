//! Legacy sensor transport documented by MousePods / Matteo Gaudioso (starman-tech).
//! https://github.com/starman-tech/mousepods/tree/79736c79ae5f86d62a137e5747b658431b8c3d9e

pub const PREFIX: [u8; 10] = [4, 0, 4, 0, 0x17, 0, 0, 8, 0, 0];
const DOCTYPE: &str = "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">";

#[derive(Debug, PartialEq, Eq)]
pub struct Packet<'a> {
    pub kind: u8,
    pub device: u8,
    pub response: bool,
    pub status: bool,
    pub timestamp: Option<u64>,
    pub payload: &'a [u8],
}

/// Validate the entire batch before allowing any announcement to change a mapping.
pub fn packets(body: &[u8]) -> Option<Vec<Packet<'_>>> {
    if body.len() < 8 || body.len() > 4096 || body[0] != 0 || body[3] & !2 != 0 {
        return None;
    }
    let mut at = 4;
    let mut out = Vec::new();
    while at < body.len() {
        if out.len() == 32 {
            return None;
        }
        let header = u32::from_le_bytes(body.get(at..at + 4)?.try_into().ok()?);
        let length = ((header >> 7) & 1023) as usize;
        let kind = ((header >> 17) & 7) as u8;
        if length < 4 || kind > 5 || header & 0xff80_0000 != 0 {
            return None;
        }
        let bytes = body.get(at + 4..at.checked_add(length)?)?;
        let stamped = header & (1 << 20) != 0;
        if stamped && (kind != 2 || bytes.len() < 9) {
            return None;
        }
        // The timestamp follows the report-type byte, not the report ID.
        let timestamp = stamped.then(|| u64::from_le_bytes(bytes[1..9].try_into().unwrap()));
        if timestamp.is_some_and(|value| value > i64::MAX as u64) {
            return None;
        }
        let payload = if stamped { bytes.get(9..)? } else { bytes };
        if kind == 2 && (!stamped && payload.first() != Some(&0) || stamped && bytes[0] != 0) {
            return None;
        }
        out.push(Packet {
            kind,
            device: (header & 127) as u8,
            response: header & (1 << 21) != 0,
            status: header & (1 << 22) != 0,
            timestamp,
            payload: if kind == 2 && !stamped {
                payload.get(1..)?
            } else {
                payload
            },
        });
        at += length;
    }
    Some(out)
}

fn frame(sequence: u16, device: u8, kind: u8, data: &[u8]) -> Vec<u8> {
    let header = u32::from(device) | (((4 + data.len()) as u32) << 7) | (u32::from(kind) << 17);
    let mut out = PREFIX.to_vec();
    out.extend_from_slice(&((8 + data.len()) as u16).to_le_bytes());
    out.extend_from_slice(&[0, sequence as u8, 0, (device >> 6) * 2]);
    out.extend_from_slice(&header.to_le_bytes());
    out.extend_from_slice(data);
    out
}

pub fn connect(sequence: u16) -> Vec<u8> {
    frame(sequence, 0, 0, &[])
}

pub fn feature(sequence: u16, device: u8, report: u8, value: u32) -> Vec<u8> {
    let mut data = vec![2, report];
    data.extend_from_slice(&value.to_le_bytes());
    frame(sequence, device, 3, &data)
}

/// Only the bounded plist subset used by sensor declarations; no entities, DTD loading or XML code.
struct Plist<'a> {
    text: &'a str,
    nodes: usize,
}
impl<'a> Plist<'a> {
    fn take(&mut self, literal: &str) -> Option<()> {
        self.text = self.text.trim_start_matches([' ', '\r', '\n', '\t']);
        self.text = self.text.strip_prefix(literal)?;
        Some(())
    }
    fn atom(&mut self, tag: &str) -> Option<&'a str> {
        self.take(&format!("<{tag}>"))?;
        let end = self.text.find('<')?;
        let value = &self.text[..end];
        if value.contains('&') {
            return None;
        }
        self.text = &self.text[end..];
        self.take(&format!("</{tag}>"))?;
        Some(value)
    }
    fn value(&mut self, depth: usize) -> Option<(&'a str, &'static str)> {
        self.nodes += 1;
        if self.nodes > 96 || depth > 4 {
            return None;
        }
        self.text = self.text.trim_start_matches([' ', '\r', '\n', '\t']);
        for tag in ["string", "integer", "data"] {
            if self.text.starts_with(&format!("<{tag}>")) {
                return Some((self.atom(tag)?, tag));
            }
        }
        for tag in ["<true/>", "<false/>"] {
            if self.text.starts_with(tag) {
                self.take(tag)?;
                return Some(("", "bool"));
            }
        }
        self.take("<array>")?;
        while !self.text.trim_start().starts_with("</array>") {
            self.value(depth + 1)?;
        }
        self.take("</array>")?;
        Some(("", "array"))
    }
}

fn base64(text: &str) -> Option<Vec<u8>> {
    let bytes: Vec<_> = text
        .bytes()
        .filter(|b| !matches!(b, b' ' | b'\r' | b'\n' | b'\t'))
        .collect();
    if bytes.is_empty() || bytes.len() > 512 || bytes.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::new();
    for (index, block) in bytes.chunks_exact(4).enumerate() {
        let mut value = 0u32;
        let mut padding = 0;
        for &b in block {
            let n = match b {
                b'A'..=b'Z' => b - b'A',
                b'a'..=b'z' => b - b'a' + 26,
                b'0'..=b'9' => b - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                b'=' => {
                    padding += 1;
                    0
                }
                _ => return None,
            };
            if padding != 0 && b != b'=' {
                return None;
            }
            value = (value << 6) | u32::from(n);
        }
        if padding > 2
            || padding != 0 && index + 1 != bytes.len() / 4
            || padding == 1 && value & 255 != 0
            || padding == 2 && value & 65535 != 0
        {
            return None;
        }
        out.extend_from_slice(&value.to_be_bytes()[1..4 - padding]);
    }
    Some(out)
}

pub fn motion_descriptor(xml: &[u8]) -> bool {
    fn parse(xml: &[u8]) -> Option<()> {
        if xml.len() > 1019 {
            return None;
        }
        let mut text = std::str::from_utf8(xml).ok()?.trim();
        if text.starts_with("<?xml ") {
            let end = text.find("?>")?;
            if end > 100 {
                return None;
            }
            text = text[end + 2..].trim_start();
        }
        if text.starts_with("<!DOCTYPE") {
            text = text.strip_prefix(DOCTYPE)?.trim_start();
        }
        let mut p = Plist { text, nodes: 0 };
        p.take("<plist version=\"1.0\">")?;
        p.take("<dict>")?;
        let mut keys = Vec::new();
        let (mut name, mut vendor, mut maximum, mut descriptor) = (None, None, None, None);
        while !p.text.trim_start().starts_with("</dict>") {
            let key = p.atom("key")?;
            if key.len() > 128 || keys.contains(&key) || keys.len() >= 24 {
                return None;
            }
            keys.push(key);
            let (value, kind) = p.value(0)?;
            match key {
                "AccessoryService" if kind == "string" => name = Some(value),
                "VendorID" if kind == "integer" => vendor = Some(value.parse::<u64>().ok()?),
                "MaxReportSize" if kind == "integer" => maximum = Some(value.parse::<u64>().ok()?),
                "ReportDescriptor" if kind == "data" => descriptor = Some(base64(value)?),
                "AccessoryService" | "VendorID" | "MaxReportSize" | "ReportDescriptor" => {
                    return None;
                }
                _ => (),
            }
        }
        p.take("</dict>")?;
        p.take("</plist>")?;
        if !p.text.trim().is_empty()
            || name != Some("devmotion6")
            || vendor != Some(1452)
            || maximum != Some(182)
            || descriptor.as_deref() != Some(super::MOTION_DESCRIPTOR)
        {
            return None;
        }
        Some(())
    }
    parse(xml).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixtures() -> serde_json::Value {
        serde_json::from_str(include_str!("../tests/fixtures/sensor-wire-v1.json")).unwrap()
    }
    fn hex(value: &serde_json::Value) -> Vec<u8> {
        value
            .as_str()
            .unwrap()
            .as_bytes()
            .chunks_exact(2)
            .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
            .collect()
    }
    #[test]
    fn public_announcements_and_synthetic_inputs_share_one_contract() {
        let f = fixtures();
        for a in f["announcements"].as_array().unwrap() {
            let frame = hex(&a["frame"]);
            let p = packets(&frame[12..]).unwrap();
            assert_eq!(p.len(), 1);
            assert_eq!(p[0].device, a["device"].as_u64().unwrap() as u8);
            assert_eq!(
                motion_descriptor(p[0].payload),
                a["service"] == "devmotion6"
            );
            for n in 0..p[0].payload.len() {
                assert!(!motion_descriptor(&p[0].payload[..n]));
            }
        }
        assert_eq!(connect(1), hex(&f["commands"]["host_connect"]));
        assert_eq!(feature(1, 7, 1, 40_000), hex(&f["commands"]["start_40ms"]));
        assert_eq!(feature(1, 7, 1, 0), hex(&f["commands"]["stop"]));
        for name in ["input", "timestamped_input"] {
            let frame = hex(&f["motion"][name]);
            let p = packets(&frame[12..]).unwrap();
            assert_eq!(p[0].payload, hex(&f["motion"]["report"]));
            assert_eq!(
                p[0].timestamp,
                (name == "timestamped_input").then_some(987654321)
            );
            for n in 0..frame.len() - 12 {
                assert!(packets(&frame[12..12 + n]).is_none());
            }
            for split in 1..frame.len() {
                let mut collector = super::super::Frames::default();
                let mut output = Vec::new();
                collector.feed_prefix(&frame[..split], &PREFIX, |b| output.push(b.to_vec()));
                assert!(output.is_empty());
                collector.feed_prefix(&frame[split..], &PREFIX, |b| output.push(b.to_vec()));
                assert_eq!(output, [frame[12..].to_vec()]);
            }
        }
    }
    #[test]
    fn malformed_batches_and_untrusted_xml_are_rejected_atomically() {
        let f = fixtures();
        let frame = hex(&f["announcements"][1]["frame"]);
        let body = &frame[12..];
        let xml = packets(body).unwrap()[0].payload;
        let text = std::str::from_utf8(xml).unwrap();
        for bad in [
            text.replace("1452", "1453"),
            text.replace("182", "191"),
            text.replace(
                "<key>VendorID</key>",
                "<key>VendorID</key><integer>1452</integer><key>VendorID</key>",
            ),
            text.replace("<string>devmotion6", "<string>&x;devmotion6"),
            text.replace(DOCTYPE, "<!DOCTYPE plist [<!ENTITY x 'bad'>]>"),
            format!("{text}<plist version=\"1.0\"></plist>"),
        ] {
            assert!(!motion_descriptor(bad.as_bytes()));
        }
        let mut b = body.to_vec();
        b.extend_from_slice(&[1, 2, 3]);
        assert!(packets(&b).is_none());
        let mut b = hex(&f["motion"]["timestamped_input"])[12..].to_vec();
        b[16] = 0x80;
        assert!(packets(&b).is_none());
        for length in [0u32, 1, 2, 3, 1023] {
            let mut b = body.to_vec();
            b[4..8].copy_from_slice(&(7 | (length << 7)).to_le_bytes());
            assert!(packets(&b).is_none());
        }
    }

    #[test]
    fn another_client_cannot_smuggle_an_embedded_sensor_frame() {
        let f = fixtures();
        let input = hex(&f["motion"]["input"]);
        let mut outer = PREFIX.to_vec();
        outer[7] = 4;
        outer.extend_from_slice(&(input.len() as u16).to_le_bytes());
        outer.extend_from_slice(&input);
        let mut collector = super::super::Frames::default();
        collector.feed_prefix(&outer, &PREFIX, |_| panic!("embedded frame"));
        let mut count = 0;
        collector.feed_prefix(&input, &PREFIX, |_| count += 1);
        assert_eq!(count, 1);
    }
}
