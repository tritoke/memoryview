use iced::{
    advanced::image::Handle,
    alignment::Vertical,
    widget::{button, column, container, image, row, slider, text},
    Element, Theme,
};
use memmap2::Mmap;
use std::fs::File;

fn main() {
    iced::application(boot, MemoryView::update, MemoryView::view)
        .theme(Theme::KanagawaWave)
        .run()
        .unwrap();
}

fn boot() -> MemoryView {
    let Some(path) = std::env::args_os().nth(1) else {
        eprintln!("Usage: ./{} <image>", std::env::args().next().unwrap());
        std::process::exit(-1);
    };

    let file = match File::open(&path) {
        Ok(file) => file,
        Err(e) => {
            eprintln!("Failed to read {path:?}: {e}");
            std::process::exit(-1);
        }
    };

    let maybe_map = unsafe { Mmap::map(&file) };
    let buf = match maybe_map {
        Ok(map) => map,
        Err(e) => {
            eprintln!("Failed to mmap file: {e}");
            std::process::exit(-1);
        }
    };

    MemoryView::new(buf)
}

struct MemoryView {
    buf: &'static Mmap,
    width: u32,
    height: u32,
    offset: usize,
}

impl MemoryView {
    fn new(buf: Mmap) -> Self {
        Self {
            buf: Box::leak(Box::new(buf)),
            width: 1920,
            height: 1080,
            offset: 0,
        }
    }

    fn update(&mut self, message: Message) {
        dbg!(&message);
        match message {
            Message::OffsetChanged(offset) => self.offset = offset,
            Message::WidthChanged(width) => self.width = width,
            Message::HeightChanged(height) => self.height = height,
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let label_width = 55;

        let offset_label = text("offset:").width(label_width);
        let offset_slider = slider(0..=self.buf.len(), self.offset, Message::OffsetChanged);
        let offset_minus =
            button("-").on_press(Message::OffsetChanged(self.offset.saturating_sub(1)));
        let offset_value = text(self.offset);
        let offset_plus =
            button("+").on_press(Message::OffsetChanged(self.buf.len().min(self.offset + 1)));
        let offset_controls = row![
            offset_label,
            offset_slider,
            offset_minus,
            offset_value,
            offset_plus
        ]
        .spacing(5)
        .align_y(Vertical::Center);

        let width_label = text("width:").width(label_width);
        let width_slider = slider(0..=10000, self.width, Message::WidthChanged);
        let width_controls = row![width_label, width_slider]
            .spacing(5)
            .align_y(Vertical::Center);

        let height_label = text("height:").width(label_width);
        let height_slider = slider(0..=10000, self.height, Message::HeightChanged);
        let height_controls = row![height_label, height_slider]
            .spacing(5)
            .align_y(Vertical::Center);

        let control_col = column![offset_controls, width_controls, height_controls].spacing(5);
        let controls = container(control_col).padding(5);
        let handle = Handle::from_rgba(self.width, self.height, &self.buf[self.offset..]);
        column![controls, image(handle)].into()
    }
}

#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone)]
enum Message {
    OffsetChanged(usize),
    WidthChanged(u32),
    HeightChanged(u32),
}
