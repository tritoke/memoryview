use iced::{
    advanced::{graphics::core::font, image::Handle},
    alignment::{Horizontal, Vertical},
    widget::{
        button, column, container, container::background, image, rich_text, row, slider, span,
        text, text_input, Row,
    },
    Color, Element, Font, Padding, Theme,
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
        fn controls<'a, T>(
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
                + num_traits::FromPrimitive
                + std::fmt::Display
                + std::str::FromStr,
            <T as std::str::FromStr>::Err: std::fmt::Debug,
            Message: Clone + 'a,
        {
            const LABEL_WIDTH: u32 = 55;

            let label = text(label_text).width(LABEL_WIDTH);
            let slider = slider(slider_range.clone(), value, on_change);

            let mut minus = button(square_bold_text("-"));
            if &value > slider_range.start() {
                minus = minus.on_press(on_change(value - 1.into()));
            }

            let value_str = format!("{value}");
            let value_text_input = text_input("", &value_str)
                .on_input(move |s| {
                    let new_value = match s.parse() {
                        Ok(new_value) => new_value,
                        Err(e) => {
                            tracing::debug!("Failed to parse input: {e:?}");
                            value
                        }
                    };
                    on_change(new_value)
                })
                .width(130);

            let mut plus = button(square_bold_text("+"));
            if &value < slider_range.end() {
                plus = plus.on_press(on_change(value + 1.into()));
            }

            row![label, slider, minus, value_text_input, plus]
                .spacing(5)
                .align_y(Vertical::Center)
        }

        let mut offset_controls = controls(
            "offset",
            self.offset_range(),
            self.offset,
            Message::OffsetChanged,
        );

        let mut skip_left_button = button(square_bold_text("<"));
        if self.offset != 0 {
            skip_left_button = skip_left_button.on_press(Message::OffsetChanged(
                self.offset.saturating_sub(self.image_size_bytes()),
            ));
        }

        let mut skip_right_button = button(square_bold_text(">"));
        if self.offset != self.offset_max() {
            skip_right_button = skip_right_button.on_press(Message::OffsetChanged(
                self.offset.saturating_add(self.image_size_bytes()),
            ));
        }

        offset_controls = offset_controls
            .push(skip_left_button)
            .push(skip_right_button);

        let width_controls = controls(
            "width",
            self.width_range(),
            self.width,
            Message::WidthChanged,
        )
        .padding(Padding {
            right: 80.0,
            ..Padding::default()
        });
        let height_controls = controls(
            "height",
            self.height_range(),
            self.height,
            Message::HeightChanged,
        )
        .padding(Padding {
            right: 80.0,
            ..Padding::default()
        });

        let control_col = column![offset_controls, width_controls, height_controls].spacing(5);
        let controls: Element<_> = container(control_col).padding(5).into();
        let handle = Handle::from_rgba(self.width, self.height, &self.buf[self.offset..]);
        let img = container(image(handle)).style(|_| background(Color::BLACK));
        column![controls, img].into()
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

    fn image_size_bytes(&self) -> usize {
        (self.width as usize)
            .saturating_mul(self.height as usize)
            .saturating_mul(4) // TODO: change when the pixel format changes
    }

    fn offset_max(&self) -> usize {
        self.buf.len().saturating_sub(self.image_size_bytes())
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

fn square_bold_text(text: &str) -> Element<'_, Message> {
    rich_text!(span(text).font(Font {
        weight: font::Weight::Bold,
        ..Font::default()
    }))
    .size(20)
    .width(15)
    .align_x(Horizontal::Center)
    .align_y(Vertical::Center)
    .on_link_click(iced::never)
    .into()
}
