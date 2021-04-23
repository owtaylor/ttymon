use vte::{Params, Parser, Perform};

pub struct Filter {
    parser: Parser,
    state: FilterState,
}

const BEL: u8 = 0x7;
const ESC: u8 = 0x1b;
const DCS: [u8; 2] = [ESC, b'P'];
const CSI: [u8; 2] = [ESC, b'['];
const OSC: [u8; 2] = [ESC, b']'];
const ST: [u8; 2] = [ESC, b'\\'];

impl Filter {
    pub fn new() -> Filter {
        Filter {
            parser: Parser::new(),
            state: FilterState::new(),
        }
    }

    pub fn fill(&mut self, buffer: &[u8]) {
        for c in buffer {
            self.parser.advance(&mut self.state, *c);
        }
    }

    #[allow(dead_code)]
    pub fn current_directory(&self) -> &str {
        &self.state.current_directory
    }

    pub fn in_window_title(&self) -> &str {
        &self.state.in_window_title
    }

    pub fn set_out_window_title(&mut self, title: &str) {
        self.state.set_out_window_title(title);
    }

    pub fn buffer(&self) -> &[u8] {
        return &self.state.buffer;
    }

    pub fn clear_buffer(&mut self) {
        self.state.buffer.clear();
    }
}

struct FilterState {
    buffer: Vec<u8>,
    current_directory: String,
    in_window_title: String,
    out_window_title: String,
    out_window_title_pending: bool,
    in_dcs: bool,
}

impl FilterState {
    fn new() -> FilterState {
        FilterState {
            buffer: vec![],
            current_directory: String::new(),
            in_window_title: String::from("ttymon"),
            out_window_title: String::new(),
            out_window_title_pending: false,
            in_dcs: false,
        }
    }

    fn set_out_window_title(&mut self, title: &str) {
        if self.out_window_title != title {
            self.out_window_title = String::from(title);
            if self.in_dcs {
                self.out_window_title_pending = true;
            } else {
                self.append_window_title(title);
            }
        }
    }

    #[inline]
    fn append(&mut self, byte: u8) {
        self.buffer.push(byte);
    }

    #[inline]
    fn append_many(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.append(*b);
        }
    }

    fn append_u16(&mut self, val: u16) {
        let mut divisor = 1u16;
        let val_over_10 = val / 10;
        while divisor <= val_over_10 {
            divisor *= 10;
        }

        let mut v = val;
        while divisor != 0 {
            let digit = v / divisor;
            self.append(b'0' + digit as u8);
            v -= digit * divisor;
            divisor /= 10;
        }
    }

    fn append_params(&mut self, params: &Params) {
        for (i, param) in params.iter().enumerate() {
            if i != 0 {
                self.append(b';');
            }

            for (i, subparam) in param.iter().enumerate() {
                if i != 0 {
                    self.append(b';');
                }

                self.append_u16(*subparam);
            }
        }
    }

    fn append_window_title(&mut self, title: &str) {
        self.append_many(&OSC);
        self.append_many(b"0;");
        self.append_many(title.as_bytes());
        self.append_many(&ST);
    }
}

impl Perform for FilterState {
    fn print(&mut self, c: char) {
        let mut b = [0; 4];
        let result = c.encode_utf8(&mut b);
        self.append_many(result.as_bytes());
    }

    fn execute(&mut self, byte: u8) {
        self.append(byte);
    }

    fn hook(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        self.in_dcs = true;
        self.append_many(&DCS);
        self.append_params(params);
        self.append_many(intermediates);
        self.append(action as u8);
    }

    fn put(&mut self, byte: u8) {
        self.append(byte);
    }

    fn unhook(&mut self) {
        self.in_dcs = false;
        self.append_many(&ST);
        if self.out_window_title_pending {
            // Copy here because rustc doesn't know that append_window_title()
            // doesn't modify self.out_window_title
            let out_window_title = self.out_window_title.clone();
            self.append_window_title(&out_window_title);
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], bell_terminated: bool) {
        if params.len() == 2 && params[0] == b"0" {
            if let Ok(title) = std::str::from_utf8(params[1]) {
                self.in_window_title = title.to_string();
            }
            return;
        }

        self.append_many(&OSC);
        for (i, param) in params.iter().enumerate() {
            if i != 0 {
                self.append(b';');
            }
            self.append_many(&param)
        }
        if bell_terminated {
            self.append(BEL);
        } else {
            self.append_many(&ST);
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        self.append_many(&CSI);
        self.append_params(params);
        self.append_many(intermediates);
        self.append(action as u8);
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        self.append(ESC);
        self.append_many(intermediates);
        self.append(byte);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_csi() {
        let mut filter = Filter::new();
        filter.fill(b"\x1b[105m\x1b[0m");
        assert_eq!(filter.buffer(), b"\x1b[105m\x1b[0m");
    }
}
