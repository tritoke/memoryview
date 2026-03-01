use iced::{
    advanced::image::Handle,
    alignment::Vertical,
    widget::{button, column, container, image, row, slider, text, Row},
    Color, Element, Theme,
};
use memmap2::Mmap;
use std::{
    fs::File,
    ops::{Add, RangeInclusive, Sub},
};

fn main() {
    tracing_subscriber::fmt::init();

    iced::application(boot, MemoryView::update, MemoryView::view)
        .theme(Theme::Oxocarbon)
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
    fn update(&mut self, message: Message) {
        tracing::debug!("{message:?}");

        match message {
            Message::OffsetChanged(offset) => self.offset = offset,
            Message::WidthChanged(width) => self.width = width,
            Message::HeightChanged(height) => self.height = height,
        }

        self.clamp_values();
    }

    fn view(&self) -> Element<'_, Message> {
        fn controls<'a, T, Message>(
            label_text: &'a str,
            slider_range: RangeInclusive<T>,
            value: T,
            on_change: impl Fn(T) -> Message + 'a + Copy,
        ) -> Row<'a, Message>
        where
            T: Copy
                + Add<Output = T>
                + Sub<Output = T>
                + From<u8>
                + PartialOrd
                + iced::advanced::text::IntoFragment<'a>
                + num_traits::AsPrimitive<f64>
                + num_traits::FromPrimitive,
            Message: Clone + 'a,
        {
            const LABEL_WIDTH: u32 = 55;

            let label = text(label_text).width(LABEL_WIDTH);
            let slider = slider(slider_range.clone(), value, on_change);

            // TODO: Use icons here
            let mut minus = button(text("-").width(15));
            if &value > slider_range.start() {
                minus = minus.on_press(on_change(value - 1.into()));
            }
            let value_text = text(value);
            let mut plus = button("+");
            if &value < slider_range.end() {
                plus = plus.on_press(on_change(value + 1.into()));
            }

            row![label, slider, minus, value_text, plus]
                .spacing(5)
                .align_y(Vertical::Center)
        }

        let offset_controls = controls(
            "offset:",
            self.offset_range(),
            self.offset,
            Message::OffsetChanged,
        );
        let width_controls = controls(
            "width:",
            self.width_range(),
            self.width,
            Message::WidthChanged,
        );
        let height_controls = controls(
            "height:",
            self.height_range(),
            self.height,
            Message::HeightChanged,
        );

        let control_col = column![offset_controls, width_controls, height_controls].spacing(5);
        let controls: Element<_> = container(control_col).padding(5).into();
        let handle = Handle::from_rgba(self.width, self.height, &self.buf[self.offset..]);
        column![controls.explain(Color::WHITE), image(handle)].into()
    }
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

    fn offset_max(&self) -> usize {
        let image_size_bytes = (self.width as usize)
            .saturating_mul(self.height as usize)
            .saturating_mul(4); // TODO: change when the pixel format changes
        self.buf.len().saturating_sub(image_size_bytes)
    }

    fn offset_range(&self) -> RangeInclusive<usize> {
        0..=self.offset_max()
    }

    fn width_range(&self) -> RangeInclusive<u32> {
        0..=u32::max(10000, self.width + 1)
    }

    fn height_range(&self) -> RangeInclusive<u32> {
        0..=u32::max(10000, self.height + 1)
    }

    fn clamp_values(&mut self) {
        if !self.offset_range().contains(&self.offset) {
            tracing::debug!(
                "Clamping offset from {} to {}",
                self.offset,
                self.offset_max()
            );
            self.offset = self.offset_max();
        }

        if self.width < 1 {
            self.width = 1;
        }

        if self.height < 1 {
            self.height = 1;
        }
    }
}

#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone)]
enum Message {
    OffsetChanged(usize),
    WidthChanged(u32),
    HeightChanged(u32),
}
