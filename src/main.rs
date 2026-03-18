use ::image as img;
use iced::{
    Color, Element, Function, Padding, Subscription, Task, Theme,
    advanced::{graphics::core::Bytes, image::Handle},
    alignment::Vertical,
    keyboard,
    widget::{
        Row, button, checkbox, column, container, image, pick_list, right, row, slider, space,
        text, text_input,
    },
};
use lucide_icons::Icon;
use memmap2::Mmap;
use rounded_div::RoundedDiv;
use strum::{EnumIter, VariantArray};

use std::{
    collections::BTreeMap,
    fs::File,
    hint::unreachable_unchecked,
    mem,
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
    let (_, regen_image_task) = state.regen_image();
    (state, regen_image_task)
}

static HANDLE_NO: AtomicU64 = AtomicU64::new(1);

struct MemoryView {
    buf: &'static Mmap,
    width: u32,
    height: u32,
    offset: usize,
    pixel_format: PixelFormat,
    swap_bytes: bool,
    scale_factor: f32,
    view: Option<image::Allocation>,
    view_no: u64,
    handles: BTreeMap<u64, iced::task::Handle>,
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
            Message::ByteSwap(swap_bytes) => self.swap_bytes = swap_bytes,
            Message::ScaleDecrease => self.scale_factor *= 0.8,
            Message::ScaleIncrease => self.scale_factor *= 1.25,
            Message::ScaleReset => self.scale_factor = 1.0,
            Message::NewImage(view_no, Ok(allocation)) => {
                if view_no > self.view_no {
                    self.view = Some(allocation);
                    self.view_no = view_no;
                }

                // cancel any tasks which are older than the current completed view
                self.handles.retain(|&handle_no, _| handle_no > view_no);
            }
            Message::SaveImage => {
                return self.save_image();
            }
            Message::SaveImageResult(Ok(())) => {
                tracing::debug!("Successfully saved image");
                // TODO: show a toast to say the file was saved
                return Task::none();
            }
            Message::NewImage(_, Err(e)) => {
                tracing::error!("Failed generating new image: {e}");
                // TODO: maybe show a toast to say we fucked it?
                return Task::none();
            }
            Message::SaveImageResult(Err(e)) => {
                tracing::error!("Failed saving image: {e}");
                // TODO: show a toast to say we fucked it
            }
        }

        self.clamp_values();

        if needs_regen {
            let (handle_no, task) = self.regen_image();
            let (task, mut handle) = task.abortable();
            handle = handle.abort_on_drop();
            self.handles.insert(handle_no, handle);
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
            "Swap byte-order",
            swap_bytes,
            right(save_button)
        ]
        .spacing(5)
        .align_y(Vertical::Center);

        // TODO: Add endianness control for >8 bit formats
        // TODO: Add data type controls, i.e. signed, unsigned, float

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
            pixel_format: PixelFormat::Rgb8,
            swap_bytes: false,
            view_no: 0,
            view: None,
            scale_factor: 1.0,
            handles: BTreeMap::new(),
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

    fn regen_image(&self) -> (u64, Task<Message>) {
        let params = HandleGenParams {
            width: self.width,
            height: self.height,
            offset: self.offset,
            format: self.pixel_format,
            swap_bytes: self.swap_bytes,
        };

        let handle_no = HANDLE_NO.fetch_add(1, Ordering::Relaxed);
        let data = &self.buf[..];

        // generate the new image on a blocking tokio thread
        let task = Task::future(async move {
            tokio::task::spawn_blocking(move || Self::generate_new_image_handle(data, params))
                .await
                .expect("Failed to await blocking thread")
        })
        .then(image::allocate)
        .map(Message::NewImage.with(handle_no));

        (handle_no, task)
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
#[derive(Debug, Clone)]
enum Message {
    OffsetChanged(usize),
    WidthChanged(u32),
    HeightChanged(u32),
    FormatChanged(PixelFormat),
    ByteSwap(bool),
    ScaleIncrease,
    ScaleDecrease,
    ScaleReset,
    NewImage(u64, Result<image::Allocation, image::Error>),
    SaveImage,
    SaveImageResult(Result<(), String>),
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
    const fn size(&self) -> usize {
        match self {
            Self::Bgr8 | Self::Rgb8 => 3,
            Self::Bgr16 | Self::Rgb16 => 6,
            Self::Bgr32 | Self::Rgb32 => 12,
            Self::Bgra8 | Self::Rgba8 => 4,
            Self::Bgra16 | Self::Rgba16 => 8,
            Self::Bgra32 | Self::Rgba32 => 16,
            Self::Rgb565 | Self::Bgr565 => 2,
        }
    }
}

impl std::fmt::Display for PixelFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
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
    iced::widget::text(char::from(icon).to_string()).font(iced::Font::with_name("lucide"))
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

impl SwapBytesCond for u8 {
    fn swap_bytes_cond(self, _swap: bool) -> Self {
        self
    }
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
