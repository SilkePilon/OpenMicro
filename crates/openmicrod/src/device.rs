use async_trait::async_trait;
use openmicro_proto::LedFrame;

#[async_trait]
pub trait DeviceLink: Send {
    async fn set_leds(&mut self, frame: &LedFrame);
    #[allow(dead_code)]
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

#[async_trait]
impl DeviceLink for MockDevice {
    async fn set_leds(&mut self, frame: &LedFrame) {
        self.last = *frame;
        self.writes += 1;
    }
    fn last_frame(&self) -> LedFrame {
        self.last
    }
}
