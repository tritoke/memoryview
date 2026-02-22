use iced::{
    advanced::image::Handle,
    widget::{button, column, image},
    Element,
};
use memmap2::Mmap;
use std::fs::File;

fn main() {
    iced::application(boot, MemoryView::update, MemoryView::view)
        .run()
        .unwrap();
}

fn boot() -> MemoryView {
    let Some(path) = std::env::args_os().nth(1) else {
        eprintln!("Usage: ./{} <image>", std::env::args().next().unwrap());
        std::process::exit(-1);
    };

    dbg!(&path);

    let file = match File::open(&path) {
        Ok(file) => file,
        Err(e) => {
            eprintln!("Failed to read {path:?}: {e}");
            std::process::exit(-1);
        }
    };

    dbg!(&file);

    let maybe_map = unsafe { Mmap::map(&file) };
    let buf = match maybe_map {
        Ok(map) => map,
        Err(e) => {
            eprintln!("Failed to mmap file: {e}");
            std::process::exit(-1);
        }
    };

    dbg!(buf.len());

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
            width: 100,
            height: 100,
            offset: 0,
        }
    }

    fn update(&mut self, message: Message) {
        println!("update called: message = {message:?}");
    }

    fn view(&self) -> Element<'_, Message> {
        let handle = Handle::from_rgba(self.width, self.height, &self.buf[self.offset..]);

        image(handle).into()
    }
}

#[derive(Debug, Clone)]
enum Message {}
