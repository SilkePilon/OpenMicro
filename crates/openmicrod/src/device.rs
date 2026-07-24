use openmicro_proto::LedFrame;

pub trait DeviceLink: Send {
    fn set_leds(&mut self, frame: &LedFrame);
    fn last_frame(&self) -> LedFrame;
}

#[derive(Debug)]
pub struct MockDevice {
    last: LedFrame,
    pub writes: usize,
}

impl MockDevice {
    pub fn new() -> Self {
        Self { last: LedFrame::BLANK, writes: 0 }
    }
}

impl DeviceLink for MockDevice {
    fn set_leds(&mut self, frame: &LedFrame) {
        self.last = *frame;
        self.writes += 1;
    }
    fn last_frame(&self) -> LedFrame {
        self.last
    }
}
