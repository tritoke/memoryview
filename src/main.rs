use std::{
    fs::File,
    hint::unreachable_unchecked,
    mem,
    ops::{Add, RangeInclusive, Sub},
};

use ::image as img;
use iced::{
    Alignment, Color, Element, Length, Subscription, Task, Theme,
    advanced::{graphics::core::Bytes, image::Handle},
    alignment::Vertical,
    keyboard,
    widget::{
        Row, Tooltip, button, checkbox, column, container, image, pick_list, right, row, slider,
        space, text, text_input, tooltip,
    },
};
use lucide_icons::Icon;
use memmap2::Mmap;
use rounded_div::RoundedDiv;
use strum::{EnumIter, VariantArray};

mod toasts;
use toasts::{Status, Toast};

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

fn boot() -> (MemoryView, Task<Message>) {
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

    let state = MemoryView::new(buf);
    let regen_image_task = state.regen_image();
    (state, regen_image_task)
}

struct MemoryView {
    buf: &'static Mmap,
    width: u32,
    height: u32,
    offset: usize,
    pixel_format: PixelFormat,
    swap_bytes: bool,
    scale_factor: f32,
    view: Option<image::Allocation>,
    image_regen_task: Option<iced::task::Handle>,
    toasts: Vec<Toast>,
}

impl MemoryView {
    fn update(&mut self, message: Message) -> Task<Message> {
        let needs_regen = message.invalidates_image();

        match message {
            Message::OffsetChanged(offset) => self.offset = offset,
            Message::WidthChanged(width) => self.width = width,
            Message::HeightChanged(height) => self.height = height,
            Message::FormatChanged(format) => self.pixel_format = format,
            Message::ByteSwap(swap_bytes) => self.swap_bytes = swap_bytes,
            Message::ScaleDecrease => self.scale_factor *= 0.8,
            Message::ScaleIncrease => self.scale_factor *= 1.25,
            Message::ScaleReset => self.scale_factor = 1.0,
            Message::NewImage(Ok(allocation)) => {
                self.view = Some(allocation);
            }
            Message::SaveImage => {
                return self.save_image();
            }
            Message::SaveImageResult(Ok(())) => {
                tracing::debug!("Successfully saved image");
                self.toasts.push(Toast {
                    title: String::from("memoryview"),
                    body: String::from("Saved image to image.png"),
                    status: Status::Success,
                });
                return Task::none();
            }
            Message::NewImage(Err(e)) => {
                tracing::error!("Failed generating new image: {e}");
                self.toasts.push(Toast {
                    title: String::from("memoryview"),
                    body: format!("Failed to generate an image: {e}"),
                    status: Status::Warning,
                });
                return Task::none();
            }
            Message::SaveImageResult(Err(e)) => {
                tracing::error!("Failed saving image: {e}");
                self.toasts.push(Toast {
                    title: String::from("memoryview"),
                    body: format!("Failed to save the image: {e}"),
                    status: Status::Warning,
                });
            }
            Message::CloseToast(idx) => {
                self.toasts.remove(idx);
                return Task::none();
            }
        }

        self.clamp_values();

        if needs_regen {
            let task = self.regen_image();
            let (task, handle) = task.abortable();
            self.image_regen_task = Some(handle.abort_on_drop());
            task
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
        }

        let mut skip_line_left_button = button(icon(Icon::ChevronLeft));
        if self.offset != 0 {
            skip_line_left_button = skip_line_left_button.on_press(Message::OffsetChanged(
                self.offset.saturating_sub(self.row_size()),
            ));
        }

        let mut skip_line_right_button = button(icon(Icon::ChevronRight));
        if self.offset != self.offset_max() {
            skip_line_right_button = skip_line_right_button.on_press(Message::OffsetChanged(
                self.offset.saturating_add(self.row_size()),
            ));
        }

        let mut skip_whole_left_button = button(icon(Icon::ChevronsLeft));
        if self.offset != 0 {
            skip_whole_left_button = skip_whole_left_button.on_press(Message::OffsetChanged(
                self.offset.saturating_sub(self.image_size()),
            ));
        }

        let mut skip_whole_right_button = button(icon(Icon::ChevronsRight));
        if self.offset != self.offset_max() {
            skip_whole_right_button = skip_whole_right_button.on_press(Message::OffsetChanged(
                self.offset.saturating_add(self.image_size()),
            ));
        }

        fn tooltip_below<'a, Message>(
            content: impl Into<Element<'a, Message>>,
            msg: impl Into<Element<'a, Message>>,
        ) -> Tooltip<'a, Message> {
            tooltip(content.into(), msg, tooltip::Position::Bottom)
                .delay(std::time::Duration::from_millis(500))
        }

        let skip_controls = row![
            space::horizontal().width(Length::Fill),
            tooltip_below(skip_whole_left_button, "Skip one image backward"),
            tooltip_below(skip_line_left_button, "Skip one row backward"),
            tooltip_below(skip_line_right_button, "Skip one row forward"),
            tooltip_below(skip_whole_right_button, "Skip one image forward"),
        ]
        .spacing(5);

        let offset_controls = controls(
            "offset",
            self.offset_range(),
            self.offset,
            Message::OffsetChanged,
        );

        let width_controls = controls(
            "width",
            self.width_range(),
            self.width,
            Message::WidthChanged,
        );
        let height_controls = controls(
            "height",
            self.height_range(),
            self.height,
            Message::HeightChanged,
        );

        let format_picker = pick_list(
            Some(self.pixel_format),
            PixelFormat::VARIANTS,
            PixelFormat::to_string,
        )
        .on_select(Message::FormatChanged);

        let swap_bytes = checkbox(self.swap_bytes).on_toggle(Message::ByteSwap);

        let mut save_button = button("save");
        if self.view.is_some() {
            save_button = save_button.on_press(Message::SaveImage);
        }

        let format_controls = row![
            text("format").width(LABEL_WIDTH),
            format_picker,
            space::horizontal().width(5),
            if self.pixel_format.is_bit_oriented() {
                "Swap bit-order"
            } else {
                "Swap byte-order"
            },
            swap_bytes,
            right(save_button)
        ]
        .spacing(5)
        .align_y(Vertical::Center);

        let control_col = column![
            skip_controls,
            offset_controls,
            width_controls,
            height_controls,
            format_controls
        ]
        .spacing(5)
        .padding(5);

        let mut content = column![control_col];

        if let Some(allocation) = &self.view {
            let image_with_background = container(image(allocation.handle()))
                .style(|_| iced::widget::container::background(Color::BLACK));

            let img = container(image_with_background)
                .center(Length::Fill)
                .align_x(Alignment::Center)
                .align_y(Alignment::Center);

            content = content.push(img);
        }

        toasts::Manager::new(content, &self.toasts, Message::CloseToast).into()
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
            pixel_format: PixelFormat::Rgb8,
            swap_bytes: false,
            view: None,
            scale_factor: 1.0,
            image_regen_task: None,
            toasts: Vec::new(),
        }
    }

    fn row_size(&self) -> usize {
        let mut size = (self.width as usize).saturating_mul(self.pixel_format.size_bits());
        if !self.pixel_format.is_bit_oriented() {
            size /= 8
        }
        size
    }

    fn image_size(&self) -> usize {
        let mut size = (self.width as usize)
            .saturating_mul(self.height as usize)
            .saturating_mul(self.pixel_format.size_bits());
        if !self.pixel_format.is_bit_oriented() {
            size /= 8
        }
        size
    }

    fn offset_max(&self) -> usize {
        if self.pixel_format.is_bit_oriented() {
            self.buf
                .len()
                .saturating_mul(8)
                .saturating_sub(self.image_size())
        } else {
            self.buf.len().saturating_sub(self.image_size().div_ceil(8))
        }
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

    // TODO: add tests for pixel format conversions matched against GIMP
    #[rustfmt::skip]
    fn generate_new_image_handle(buf: &'static [u8], params: HandleGenParams) -> Handle {
        // early return for the non-allocating case
        if params.format == PixelFormat::Rgba8 {
            return Handle::from_rgba(
                params.width,
                params.height,
                Bytes::from_static(&buf[params.offset..]),
            );
        }

        let mut rgba = vec![0; params.width as usize * params.height as usize * 4];
        let (rgba_pixels, _) = rgba.as_chunks_mut::<4>();
        match params.format {
            PixelFormat::Bw1 => {
                let bits: Box<dyn Iterator<Item = bool>> = if params.swap_bytes {
                    Box::new(RBiterator::from(buf))
                } else {
                    Box::new(Biterator::from(buf))
                };

                for (rgba, bit) in rgba_pixels.iter_mut().zip(bits.skip(params.offset)) {
                    if bit {
                        *rgba = [0xFF, 0xFF, 0xFF, 0xFF];
                    } else {
                        *rgba = [0x00, 0x00, 0x00, 0xFF];
                    }
                }
            }
            PixelFormat::Gr2 => {
                // TODO: when array_chunks is stable use that instead
                let bits: Box<dyn Iterator<Item = (bool, bool)>> = if params.swap_bytes {
                    let rbits = RBiterator::from(buf).skip(params.offset);
                    let rbits_even = rbits.clone().step_by(2);
                    let rbits_odd = rbits.skip(1).step_by(2);
                    Box::new(rbits_even.zip(rbits_odd))
                } else {
                    let bits = Biterator::from(buf).skip(params.offset);
                    let bits_even = bits.clone().step_by(2);
                    let bits_odd = bits.skip(1).step_by(2);
                    Box::new(bits_even.zip(bits_odd))
                };

                let greyscale = bits.map(|bit| {
                    match bit {
                        (false, false) => 0x00,
                        (false, true) => 0x55,
                        (true, false) => 0xAA,
                        (true, true) => 0xFF,
                    }
                });

                for (rgba, lux) in rgba_pixels.iter_mut().zip(greyscale) {
                    *rgba = [lux, lux, lux, 0xFF];
                }
            }
            PixelFormat::Gr4 => {
                // TODO: when array_chunks is stable use that instead
                let bits: Box<dyn Iterator<Item = (((bool, bool), bool), bool)>> = if params.swap_bytes {
                    let rbits = RBiterator::from(buf).skip(params.offset);
                    let rbits_0 = rbits.clone().step_by(4);
                    let rbits_1 = rbits.clone().skip(1).step_by(4);
                    let rbits_2 = rbits.clone().skip(2).step_by(4);
                    let rbits_3 = rbits.skip(3).step_by(4);
                    Box::new(rbits_0.zip(rbits_1).zip(rbits_2).zip(rbits_3))
                } else {
                    let bits = Biterator::from(buf).skip(params.offset);
                    let bits_0 = bits.clone().step_by(4);
                    let bits_1 = bits.clone().skip(1).step_by(4);
                    let bits_2 = bits.clone().skip(2).step_by(4);
                    let bits_3 = bits.skip(3).step_by(4);
                    Box::new(bits_0.zip(bits_1).zip(bits_2).zip(bits_3))
                };

                let greyscale = bits.map(|(((a, b), c), d)| {
                    let k = (a as u32) << 3 | (b as u32) << 2 | (c as u32) << 1 | d as u32;
                    ((255 * k) / 15) as u8
                });

                for (rgba, lux) in rgba_pixels.iter_mut().zip(greyscale) {
                    *rgba = [lux, lux, lux, 0xFF];
                }
            }
            PixelFormat::Gr8 => {
                for (rgba, &lux) in rgba_pixels.iter_mut().zip(&buf[params.offset..]) {
                    *rgba = [lux, lux, lux, 0xFF];
                }
            }
            PixelFormat::Gr16 => {
                let (greyscale_pixels, _) = buf[params.offset..].as_chunks::<2>();

                for (rgba, &luxw) in rgba_pixels.iter_mut().zip(greyscale_pixels) {
                    let lux_raw = u16::from_ne_bytes(luxw);
                    let lux = lux_raw.swap_bytes_cond(params.swap_bytes).rounded_div(0x100) as u8;
                    *rgba = [lux, lux, lux, 0xFF];
                }
            }
            PixelFormat::Gr32 => {
                let (greyscale_pixels, _) = buf[params.offset..].as_chunks::<4>();

                for (rgba, &luxw) in rgba_pixels.iter_mut().zip(greyscale_pixels) {
                    let lux_raw = u32::from_ne_bytes(luxw);
                    let lux = lux_raw.swap_bytes_cond(params.swap_bytes).rounded_div(0x1000000) as u8;
                    *rgba = [lux, lux, lux, 0xFF];
                }
            }
            PixelFormat::Rgb8 => {
                let (rgb_pixels, _) = buf[params.offset..].as_chunks::<3>();

                for (rgba, &[r, g, b]) in rgba_pixels.iter_mut().zip(rgb_pixels) {
                    *rgba = [r, g, b, 0xFF];
                }
            }
            PixelFormat::Bgr8 => {
                let (bgr_pixels, _) = buf[params.offset..].as_chunks::<3>();

                for (rgba, &[b, g, r]) in rgba_pixels.iter_mut().zip(bgr_pixels) {
                    *rgba = [r, g, b, 0xFF];
                }
            }
            PixelFormat::Rgb16 => {
                let (rgb_pixels, _) = buf[params.offset..].as_chunks::<6>();

                for (rgba, &data) in rgba_pixels.iter_mut().zip(rgb_pixels) {
                    // safety: don't worry about ittttt
                    let [rw, gw, bw] = unsafe { mem::transmute::<[u8; 6], [u16; 3]>(data) };

                    let r = rw.swap_bytes_cond(params.swap_bytes).rounded_div(0x100) as u8;
                    let g = gw.swap_bytes_cond(params.swap_bytes).rounded_div(0x100) as u8;
                    let b = bw.swap_bytes_cond(params.swap_bytes).rounded_div(0x100) as u8;
                    *rgba = [r, g, b, 0xFF];
                }
            }
            PixelFormat::Bgr16 => {
                let (bgr_pixels, _) = buf[params.offset..].as_chunks::<6>();

                for (rgba, &data) in rgba_pixels.iter_mut().zip(bgr_pixels) {
                    // safety: don't worry about ittttt
                    let [bw, gw, rw] = unsafe { mem::transmute::<[u8; 6], [u16; 3]>(data) };

                    let r = rw.swap_bytes_cond(params.swap_bytes).rounded_div(0x100) as u8;
                    let g = gw.swap_bytes_cond(params.swap_bytes).rounded_div(0x100) as u8;
                    let b = bw.swap_bytes_cond(params.swap_bytes).rounded_div(0x100) as u8;
                    *rgba = [r, g, b, 0xFF];
                }
            }
            PixelFormat::Rgb32 => {
                let (rgb_pixels, _) = buf[params.offset..].as_chunks::<12>();

                for (rgba, &data) in rgba_pixels.iter_mut().zip(rgb_pixels) {
                    // safety: don't worry about ittttt
                    let [rw, gw, bw] = unsafe { mem::transmute::<[u8; 12], [u32; 3]>(data) };

                    let r = rw.swap_bytes_cond(params.swap_bytes).rounded_div(0x1_000_000) as u8;
                    let g = gw.swap_bytes_cond(params.swap_bytes).rounded_div(0x1_000_000) as u8;
                    let b = bw.swap_bytes_cond(params.swap_bytes).rounded_div(0x1_000_000) as u8;
                    *rgba = [r, g, b, 0xFF];
                }
            }
            PixelFormat::Bgr32 => {
                let (bgr_pixels, _) = buf[params.offset..].as_chunks::<12>();

                for (rgba, &data) in rgba_pixels.iter_mut().zip(bgr_pixels) {
                    // safety: don't worry about ittttt
                    let [bw, gw, rw] = unsafe { mem::transmute::<[u8; 12], [u32; 3]>(data) };

                    let r = rw.swap_bytes_cond(params.swap_bytes).rounded_div(0x1_000_000) as u8;
                    let g = gw.swap_bytes_cond(params.swap_bytes).rounded_div(0x1_000_000) as u8;
                    let b = bw.swap_bytes_cond(params.swap_bytes).rounded_div(0x1_000_000) as u8;
                    *rgba = [r, g, b, 0xFF];
                }
            }
            PixelFormat::Bgra8 => {
                let (bgra_pixels, _) = buf[params.offset..].as_chunks::<4>();

                for (rgba, &[b, g, r, a]) in rgba_pixels.iter_mut().zip(bgra_pixels) {
                    *rgba = [r, g, b, a];
                }
            }
            PixelFormat::Rgba16 => {
                let (rgba_in_pixels, _) = buf[params.offset..].as_chunks::<8>();

                for (rgba, &data) in rgba_pixels.iter_mut().zip(rgba_in_pixels) {
                    // safety: don't worry about ittttt
                    let [rw, gw, bw, aw] = unsafe { mem::transmute::<[u8; 8], [u16; 4]>(data) };

                    let r = rw.swap_bytes_cond(params.swap_bytes).rounded_div(0x100) as u8;
                    let g = gw.swap_bytes_cond(params.swap_bytes).rounded_div(0x100) as u8;
                    let b = bw.swap_bytes_cond(params.swap_bytes).rounded_div(0x100) as u8;
                    let a = aw.swap_bytes_cond(params.swap_bytes).rounded_div(0x100) as u8;
                    *rgba = [r, g, b, a];
                }
            }
            PixelFormat::Bgra16 => {
                let (bgra_pixels, _) = buf[params.offset..].as_chunks::<8>();

                for (rgba, &data) in rgba_pixels.iter_mut().zip(bgra_pixels) {
                    // safety: don't worry about ittttt
                    let [bw, gw, rw, aw] = unsafe { mem::transmute::<[u8; 8], [u16; 4]>(data) };

                    let r = rw.swap_bytes_cond(params.swap_bytes).rounded_div(0x100) as u8;
                    let g = gw.swap_bytes_cond(params.swap_bytes).rounded_div(0x100) as u8;
                    let b = bw.swap_bytes_cond(params.swap_bytes).rounded_div(0x100) as u8;
                    let a = aw.swap_bytes_cond(params.swap_bytes).rounded_div(0x100) as u8;
                    *rgba = [r, g, b, a];
                }
            }
            PixelFormat::Rgba32 => {
                let (rgba_in_pixels, _) = buf[params.offset..].as_chunks::<16>();

                for (rgba, &data) in rgba_pixels.iter_mut().zip(rgba_in_pixels) {
                    // safety: don't worry about ittttt
                    let [rw, gw, bw, aw] = unsafe { mem::transmute::<[u8; 16], [u32; 4]>(data) };

                    let r = rw.swap_bytes_cond(params.swap_bytes).rounded_div(0x1_000_000) as u8;
                    let g = gw.swap_bytes_cond(params.swap_bytes).rounded_div(0x1_000_000) as u8;
                    let b = bw.swap_bytes_cond(params.swap_bytes).rounded_div(0x1_000_000) as u8;
                    let a = aw.swap_bytes_cond(params.swap_bytes).rounded_div(0x1_000_000) as u8;
                    *rgba = [r, g, b, a];
                }
            }
            PixelFormat::Bgra32 => {
                let (bgra_pixels, _) = buf[params.offset..].as_chunks::<16>();

                for (rgba, &data) in rgba_pixels.iter_mut().zip(bgra_pixels) {
                    // safety: don't worry about ittttt
                    let [bw, gw, rw, aw] = unsafe { mem::transmute::<[u8; 16], [u32; 4]>(data) };

                    let r = rw.swap_bytes_cond(params.swap_bytes).rounded_div(0x1_000_000) as u8;
                    let g = gw.swap_bytes_cond(params.swap_bytes).rounded_div(0x1_000_000) as u8;
                    let b = bw.swap_bytes_cond(params.swap_bytes).rounded_div(0x1_000_000) as u8;
                    let a = aw.swap_bytes_cond(params.swap_bytes).rounded_div(0x1_000_000) as u8;
                    *rgba = [r, g, b, a];
                }
            }
            PixelFormat::Rgb565 => {
                let (rgb_in_pixels, _) = buf[params.offset..].as_chunks::<2>();

                for (rgba, &data) in rgba_pixels.iter_mut().zip(rgb_in_pixels) {
                    const FIVE: u16 = 0b11111;
                    const SIX: u16 = 0b111111;

                    let pix = u16::from_le_bytes(data);
                    let r = ((pix >> 11) * 255).rounded_div(FIVE) as u8;
                    let g = (((pix >> 5) & SIX) * 255).rounded_div(SIX) as u8;
                    let b = ((pix & FIVE) * 255).rounded_div(FIVE) as u8;
                    *rgba = [r, g, b, 0xFF];
                }
            }
            PixelFormat::Bgr565 => {
                let (rgb_in_pixels, _) = buf[params.offset..].as_chunks::<2>();

                for (rgba, &data) in rgba_pixels.iter_mut().zip(rgb_in_pixels) {
                    const FIVE: u16 = 0b11111;
                    const SIX: u16 = 0b111111;

                    let pix = u16::from_le_bytes(data);
                    let b = ((pix >> 11) * 255).rounded_div(FIVE) as u8;
                    let g = (((pix >> 5) & SIX) * 255).rounded_div(SIX) as u8;
                    let r = ((pix & FIVE) * 255).rounded_div(FIVE) as u8;
                    *rgba = [r, g, b, 0xFF];
                }
            }
            PixelFormat::Rgba8 => unsafe { unreachable_unchecked() },
        };

        Handle::from_rgba(params.width, params.height, Bytes::from_owner(rgba))
    }

    fn regen_image(&self) -> Task<Message> {
        let params = HandleGenParams {
            width: self.width,
            height: self.height,
            offset: self.offset,
            format: self.pixel_format,
            swap_bytes: self.swap_bytes,
        };

        let data = &self.buf[..];

        // generate the new image on a blocking tokio thread
        Task::future(async move {
            tokio::task::spawn_blocking(move || Self::generate_new_image_handle(data, params))
                .await
                .expect("Failed to await blocking thread")
        })
        .then(image::allocate)
        .map(Message::NewImage)
    }

    fn save_image(&self) -> Task<Message> {
        let Some(allocation) = &self.view else {
            return Task::none();
        };

        let Handle::Rgba {
            width,
            height,
            pixels,
            ..
        } = allocation.handle().clone()
        else {
            unreachable!("non rgba handle???")
        };

        Task::perform(save_image(pixels, width, height), Message::SaveImageResult)
    }
}

async fn save_image(pixels: Bytes, width: u32, height: u32) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        img::save_buffer(
            "image.png",
            &pixels[..],
            width,
            height,
            img::ColorType::Rgba8,
        )
        .map_err(|e| e.to_string())
    })
    .await
    .expect("Failed to await blocking task")
}

#[allow(clippy::enum_variant_names)]
#[derive(Clone)]
enum Message {
    OffsetChanged(usize),
    WidthChanged(u32),
    HeightChanged(u32),
    FormatChanged(PixelFormat),
    ByteSwap(bool),
    ScaleIncrease,
    ScaleDecrease,
    ScaleReset,
    NewImage(Result<image::Allocation, image::Error>),
    SaveImage,
    SaveImageResult(Result<(), String>),
    CloseToast(usize),
}

impl Message {
    fn invalidates_image(&self) -> bool {
        matches!(
            self,
            Self::WidthChanged(_)
                | Self::HeightChanged(_)
                | Self::OffsetChanged(_)
                | Self::FormatChanged(_)
                | Self::ByteSwap(_)
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, EnumIter, VariantArray)]
enum PixelFormat {
    Bw1,
    Gr2,
    Gr4,
    Gr8,
    Gr16,
    Gr32,
    Rgb8,
    Rgb16,
    Rgb32,
    Rgba8,
    Rgba16,
    Rgba32,
    Bgr8,
    Bgr16,
    Bgr32,
    Bgra8,
    Bgra16,
    Bgra32,
    Rgb565,
    Bgr565,
}

impl PixelFormat {
    const fn size_bits(&self) -> usize {
        match self {
            Self::Bw1 => 1,
            Self::Gr2 => 2,
            Self::Gr4 => 4,
            Self::Gr8 => 8,
            Self::Gr16 => 16,
            Self::Gr32 => 32,
            Self::Bgr8 | Self::Rgb8 => 3 * 8,
            Self::Bgr16 | Self::Rgb16 => 6 * 8,
            Self::Bgr32 | Self::Rgb32 => 12 * 8,
            Self::Bgra8 | Self::Rgba8 => 4 * 8,
            Self::Bgra16 | Self::Rgba16 => 8 * 8,
            Self::Bgra32 | Self::Rgba32 => 16 * 8,
            Self::Rgb565 | Self::Bgr565 => 2 * 8,
        }
    }

    const fn is_bit_oriented(&self) -> bool {
        self.size_bits() < 8
    }
}

impl std::fmt::Display for PixelFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PixelFormat::Bw1 => "B&W 1-bit",
            PixelFormat::Gr2 => "Greyscale 2-bit",
            PixelFormat::Gr4 => "Greyscale 4-bit",
            PixelFormat::Gr8 => "Greyscale 8-bit",
            PixelFormat::Gr16 => "Greyscale 16-bit",
            PixelFormat::Gr32 => "Greyscale 32-bit",
            PixelFormat::Rgb8 => "RGB 8-bit",
            PixelFormat::Rgb16 => "RGB 16-bit",
            PixelFormat::Rgb32 => "RGB 32-bit",
            PixelFormat::Rgba8 => "RGBA 8-bit",
            PixelFormat::Rgba16 => "RGBA 16-bit",
            PixelFormat::Rgba32 => "RGBA 32-bit",
            PixelFormat::Bgr8 => "BGR 8-bit",
            PixelFormat::Bgr16 => "BGR 16-bit",
            PixelFormat::Bgr32 => "BGR 32-bit",
            PixelFormat::Bgra8 => "BGRA 8-bit",
            PixelFormat::Bgra16 => "BGRA 16-bit",
            PixelFormat::Bgra32 => "BGRA 32-bit",
            PixelFormat::Rgb565 => "RGB565",
            PixelFormat::Bgr565 => "BGR565",
        })
    }
}

fn icon<'a>(icon: Icon) -> iced::widget::Text<'a> {
    iced::widget::text(char::from(icon).to_string()).font(iced::Font::with_family("lucide"))
}

#[derive(Copy, Clone, Debug)]
struct HandleGenParams {
    offset: usize,
    width: u32,
    height: u32,
    format: PixelFormat,
    swap_bytes: bool,
}

trait SwapBytesCond {
    fn swap_bytes_cond(self, swap: bool) -> Self;
}

impl SwapBytesCond for u16 {
    fn swap_bytes_cond(self, swap: bool) -> Self {
        if swap { self.swap_bytes() } else { self }
    }
}

impl SwapBytesCond for u32 {
    fn swap_bytes_cond(self, swap: bool) -> Self {
        if swap { self.swap_bytes() } else { self }
    }
}

#[derive(Clone, Copy, Debug)]
struct Biterator {
    buf: &'static [u8],
    bit_offset: usize,
    byte_offset: usize,
}

impl From<&'static [u8]> for Biterator {
    fn from(buf: &'static [u8]) -> Self {
        Self {
            buf,
            bit_offset: 0,
            byte_offset: 0,
        }
    }
}

impl Iterator for Biterator {
    type Item = bool;

    fn next(&mut self) -> Option<Self::Item> {
        let shift = 7 - self.bit_offset;
        let byte = *self.buf.get(self.byte_offset)?;

        if self.bit_offset == 7 {
            self.byte_offset += 1;
            self.bit_offset = 0;
        } else {
            self.bit_offset += 1;
        }

        Some(((byte >> shift) & 1) == 1)
    }

    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        let byte_offset = n / 8;
        let bit_offset = n % 8;

        let x = self.bit_offset + bit_offset;
        self.bit_offset = x & 0b111;
        self.byte_offset = self
            .byte_offset
            .checked_add(byte_offset)
            .and_then(|n| n.checked_add(x >> 3))
            .expect("impressive, you've come a reaallly long way 😂");

        self.next()
    }
}

#[derive(Clone, Copy, Debug)]
struct RBiterator {
    buf: &'static [u8],
    bit_offset: usize,
    byte_offset: usize,
}

impl From<&'static [u8]> for RBiterator {
    fn from(buf: &'static [u8]) -> Self {
        Self {
            buf,
            bit_offset: 0,
            byte_offset: 0,
        }
    }
}

impl Iterator for RBiterator {
    type Item = bool;

    fn next(&mut self) -> Option<Self::Item> {
        let shift = self.bit_offset;
        let byte = *self.buf.get(self.byte_offset)?;

        if self.bit_offset == 7 {
            self.byte_offset += 1;
            self.bit_offset = 0;
        } else {
            self.bit_offset += 1;
        }

        Some(((byte >> shift) & 1) == 1)
    }

    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        let byte_offset = n / 8;
        let bit_offset = n % 8;

        let x = self.bit_offset + bit_offset;
        self.bit_offset = x & 0b111;
        self.bit_offset = x & 0b111;
        self.byte_offset = self
            .byte_offset
            .checked_add(byte_offset)
            .and_then(|n| n.checked_add(x >> 3))
            .expect("impressive, you've come a reaallly long way 😂");

        self.next()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_biterator() {
        let mut iter = Biterator::from(&[0xFF, 0x00, 0b11110000, 0b10101010][..]);
        for _ in 0..8 {
            assert_eq!(iter.next(), Some(true));
        }
        for _ in 0..8 {
            assert_eq!(iter.next(), Some(false));
        }
        for _ in 0..4 {
            assert_eq!(iter.next(), Some(true));
        }
        for _ in 0..4 {
            assert_eq!(iter.next(), Some(false));
        }
        for _ in 0..4 {
            assert_eq!(iter.next(), Some(true));
            assert_eq!(iter.next(), Some(false));
        }
    }

    #[test]
    fn test_biterator_nth() {
        let base_iter = Biterator::from(&[0xFF, 0x00, 0b11110000, 0b10101010][..]);

        for i in 0..8 {
            assert_eq!(base_iter.clone().nth(i), Some(true));
        }
        for i in 8..16 {
            assert_eq!(base_iter.clone().nth(i), Some(false));
        }
        for i in 16..20 {
            assert_eq!(base_iter.clone().nth(i), Some(true));
        }
        for i in 20..24 {
            assert_eq!(base_iter.clone().nth(i), Some(false));
        }
        for i in 24..32 {
            assert_eq!(base_iter.clone().nth(i), Some(i % 2 == 0));
        }
    }

    #[test]
    fn test_rbiterator() {
        let mut iter = RBiterator::from(&[0xFF, 0x00, 0b11110000, 0b10101010][..]);
        for _ in 0..8 {
            assert_eq!(iter.next(), Some(true));
        }
        for _ in 0..8 {
            assert_eq!(iter.next(), Some(false));
        }
        for _ in 0..4 {
            assert_eq!(iter.next(), Some(false));
        }
        for _ in 0..4 {
            assert_eq!(iter.next(), Some(true));
        }
        for _ in 0..4 {
            assert_eq!(iter.next(), Some(false));
            assert_eq!(iter.next(), Some(true));
        }
    }

    #[test]
    fn test_rbiterator_nth() {
        let base_iter = RBiterator::from(&[0xFF, 0x00, 0b11110000, 0b10101010][..]);

        for i in 0..8 {
            assert_eq!(base_iter.clone().nth(i), Some(true));
        }
        for i in 8..16 {
            assert_eq!(base_iter.clone().nth(i), Some(false));
        }
        for i in 16..20 {
            assert_eq!(base_iter.clone().nth(i), Some(false));
        }
        for i in 20..24 {
            assert_eq!(base_iter.clone().nth(i), Some(true));
        }
        for i in 24..32 {
            assert_eq!(base_iter.clone().nth(i), Some(i % 2 == 1));
        }
    }
}
