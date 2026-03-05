use iced::{
    Color, Element, Function, Padding, Subscription, Task, Theme,
    advanced::{graphics::core::Bytes, image::Handle},
    alignment::{Horizontal, Vertical},
    keyboard,
    widget::{Row, button, column, combo_box, container, image, row, slider, text, text_input},
};
use lucide_icons::Icon;
use memmap2::Mmap;
use strum::{EnumIter, IntoEnumIterator};

use std::{
    fs::File,
    ops::{Add, RangeInclusive, Sub},
    sync::atomic::{AtomicU64, Ordering},
};

fn main() {
    tracing_subscriber::fmt::init();

    let settings = iced::Settings {
        // add bundled font to iced
        fonts: vec![lucide_icons::LUCIDE_FONT_BYTES.into()],
        ..Default::default()
    };

    iced::application(boot, MemoryView::update, MemoryView::view)
        .settings(settings)
        .theme(Theme::Oxocarbon)
        .subscription(MemoryView::subscription)
        .scale_factor(MemoryView::scale_factor)
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

    // TODO: spawn task to generate the first image!
    MemoryView::new(buf)
}

static HANDLE_NO: AtomicU64 = AtomicU64::new(0);

struct MemoryView {
    buf: &'static Mmap,
    width: u32,
    height: u32,
    offset: usize,
    pixel_format: PixelFormat,
    pixel_format_state: combo_box::State<PixelFormat>,
    scale_factor: f32,
    view: Option<image::Allocation>,
    view_no: u64,
    // FEAT: hold handles to close?
}

impl MemoryView {
    fn update(&mut self, message: Message) -> Task<Message> {
        tracing::debug!("{message:?}");

        let needs_regen = message.invalidates_image();

        match message {
            Message::OffsetChanged(offset) => self.offset = offset,
            Message::WidthChanged(width) => self.width = width,
            Message::HeightChanged(height) => self.height = height,
            Message::FormatChanged(format) => self.pixel_format = format,
            Message::ScaleDecrease => self.scale_factor *= 0.8,
            Message::ScaleIncrease => self.scale_factor *= 1.25,
            Message::ScaleReset => self.scale_factor = 1.0,
            Message::NewImage(view_no, Ok(allocation)) => {
                if view_no > self.view_no {
                    self.view = Some(allocation);
                }
            }
            Message::NewImage(_, Err(e)) => {
                tracing::error!("Failed generating new image: {e}");
            }
        }

        self.clamp_values();

        if needs_regen {
            self.regen_image()
        } else {
            Task::none()
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        keyboard::listen().filter_map(|e| {
            let keyboard::Event::KeyPressed {
                key: keyboard::key::Key::Character(c),
                modifiers,
                ..
            } = e
            else {
                return None;
            };

            if !modifiers.control() {
                return None;
            }

            match c.as_str() {
                "-" | "_" => Some(Message::ScaleDecrease),
                "=" | "+" => Some(Message::ScaleIncrease),
                "0" | ")" => Some(Message::ScaleReset),
                _ => None,
            }
        })
    }

    fn scale_factor(&self) -> f32 {
        self.scale_factor
    }

    fn view(&self) -> Element<'_, Message> {
        const LABEL_WIDTH: u32 = 53;

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
            Message: Clone,
        {
            let label = text(label_text).width(LABEL_WIDTH);
            let slider = slider(slider_range.clone(), value, on_change);

            let mut minus = button(icon(Icon::Minus));
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

            let mut plus = button(icon(Icon::Plus));
            if &value < slider_range.end() {
                plus = plus.on_press(on_change(value + 1.into()));
            }

            row![label, slider, minus, value_text_input, plus]
                .spacing(5)
                .align_y(Vertical::Center)
                .into()
        }

        let mut offset_controls = controls(
            "offset",
            self.offset_range(),
            self.offset,
            Message::OffsetChanged,
        );

        let mut skip_left_button = button(icon(Icon::ChevronLeft));
        if self.offset != 0 {
            skip_left_button = skip_left_button.on_press(Message::OffsetChanged(
                self.offset.saturating_sub(self.image_size_bytes()),
            ));
        }

        let mut skip_right_button = button(icon(Icon::ChevronRight));
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

        let format_combo = combo_box(
            &self.pixel_format_state,
            "",
            Some(&self.pixel_format),
            Message::FormatChanged,
        )
        .width(300);

        let format_controls = row![text("format").width(LABEL_WIDTH), format_combo]
            .spacing(5)
            .align_y(Vertical::Center);

        let control_col = column![
            offset_controls,
            width_controls,
            height_controls,
            format_controls
        ]
        .spacing(5)
        .padding(5);

        let mut elems = column![control_col];

        if let Some(allocation) = &self.view {
            let img = container(image(allocation.handle()))
                .style(|_| iced::widget::container::background(Color::BLACK));

            elems = elems.push(img);
        }

        elems.into()
    }
}

impl MemoryView {
    fn new(buf: Mmap) -> Self {
        let width = 1920;
        let height = 1080;
        let buf = Box::leak(Box::new(buf));

        Self {
            buf,
            width,
            height,
            offset: 0,
            pixel_format: PixelFormat::Rgba8,
            pixel_format_state: combo_box::State::new(PixelFormat::iter().collect()),
            view_no: 0,
            view: None,
            scale_factor: 1.0,
        }
    }

    fn image_size_bytes(&self) -> usize {
        (self.width as usize)
            .saturating_mul(self.height as usize)
            .saturating_mul(self.pixel_format.size())
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

    async fn generate_new_image_handle(buf: &'static [u8], params: HandleGenParams) -> Handle {
        let rgba_bytes = match params.format {
            PixelFormat::Rgb8 => {
                let mut bytes = vec![0xFF; params.width as usize * params.height as usize * 4];

                Bytes::from_owner(bytes)
            }
            PixelFormat::Rgba8 => Bytes::from_static(&buf[params.offset..]),
        };

        Handle::from_rgba(params.width, params.height, rgba_bytes)
    }

    fn regen_image(&self) -> Task<Message> {
        let params = HandleGenParams {
            width: self.width,
            height: self.height,
            offset: self.offset,
            format: self.pixel_format,
        };

        Task::future(Self::generate_new_image_handle(&self.buf[..], params))
            .then(image::allocate)
            .map(Message::NewImage.with(HANDLE_NO.fetch_add(1, Ordering::Relaxed)))
    }
}

#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone)]
enum Message {
    OffsetChanged(usize),
    WidthChanged(u32),
    HeightChanged(u32),
    FormatChanged(PixelFormat),
    ScaleIncrease,
    ScaleDecrease,
    ScaleReset,
    NewImage(u64, Result<image::Allocation, image::Error>),
}

impl Message {
    fn invalidates_image(&self) -> bool {
        matches!(
            self,
            Self::WidthChanged(_)
                | Self::HeightChanged(_)
                | Self::FormatChanged(_)
                | Self::OffsetChanged(_)
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, EnumIter)]
enum PixelFormat {
    Rgb8,
    Rgba8,
}

impl PixelFormat {
    fn size(&self) -> usize {
        match self {
            Self::Rgb8 => 3,
            Self::Rgba8 => 4,
        }
    }
}

impl std::fmt::Display for PixelFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PixelFormat::Rgb8 => "RGB 8-bit",
            PixelFormat::Rgba8 => "RGBA 8-bit",
        })
    }
}

fn icon<'a>(icon: Icon) -> iced::widget::Text<'a> {
    iced::widget::text(char::from(icon).to_string()).font(iced::Font::with_name("lucide"))
}

#[derive(Copy, Clone, Debug)]
struct HandleGenParams {
    offset: usize,
    width: u32,
    height: u32,
    format: PixelFormat,
}
