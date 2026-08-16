use gtk::gdk;
use gtk::prelude::*;

pub const ICON_SIZE: i32 = 20;

pub fn symbolic_image(icon_name: &str, pixel_size: i32) -> gtk::Image {
    let image = gtk::Image::from_icon_name(Some(icon_name), gtk::IconSize::Button);
    image.set_pixel_size(pixel_size);
    image
}

fn symbolic_color() -> gdk::RGBA {
    let ctx = gtk::Image::new().style_context();
    let color = ctx
        .lookup_color("theme_fg_color")
        .unwrap_or_else(|| ctx.color(gtk::StateFlags::NORMAL));
    color
}

pub fn split_view_image(vertical: bool, size: i32) -> gtk::Image {
    let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, size, size).unwrap();
    let cr = cairo::Context::new(&surface).unwrap();
    let color = symbolic_color();
    cr.set_source_rgba(
        color.red().into(),
        color.green().into(),
        color.blue().into(),
        color.alpha().into(),
    );
    cr.set_line_width(1.6);

    let margin = 2.0_f64;
    let x0 = margin;
    let y0 = margin;
    let x1 = size as f64 - margin;
    let y1 = size as f64 - margin;

    cr.rectangle(x0, y0, x1 - x0, y1 - y0);
    let _ = cr.stroke();

    if vertical {
        cr.move_to(size as f64 / 2.0, y0);
        cr.line_to(size as f64 / 2.0, y1);
    } else {
        cr.move_to(x0, size as f64 / 2.0);
        cr.line_to(x1, size as f64 / 2.0);
    }
    let _ = cr.stroke();

    let pixbuf = gdk::pixbuf_get_from_surface(&surface, 0, 0, size, size).unwrap();
    gtk::Image::from_pixbuf(Some(&pixbuf))
}

pub fn icon_button(
    icon_name: Option<&str>,
    label: Option<&str>,
    tooltip: Option<&str>,
    pixel_size: i32,
    custom_image: Option<&gtk::Image>,
) -> gtk::Button {
    let btn = gtk::Button::new();
    btn.set_relief(gtk::ReliefStyle::None);
    btn.set_size_request(36, 36);
    let image = custom_image
        .cloned()
        .unwrap_or_else(|| symbolic_image(icon_name.unwrap_or(""), pixel_size));
    match label {
        Some(text) => {
            let box_ = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            box_.pack_start(&image, false, false, 0);
            box_.pack_start(&gtk::Label::new(Some(text)), false, false, 0);
            btn.add(&box_);
        }
        None => {
            btn.add(&image);
        }
    }
    if let Some(tip) = tooltip {
        btn.set_tooltip_text(Some(tip));
    }
    btn
}
