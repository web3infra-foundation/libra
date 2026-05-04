pub struct Widget {
    name: String,
}

impl Widget {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }

    fn label(&self) -> &str {
        &self.name
    }
}

pub fn make_widget(name: &str) -> Widget {
    Widget::new(name)
}

fn handle() {}

mod nested {
    pub fn handle() {}
}
