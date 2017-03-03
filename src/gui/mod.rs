use unsegen;
use gdbmi;

use unsegen::{
    VerticalLayout,
    Widget,
    Demand,
    Window,
    Event,
    Input,
    Key,
    SeparatingStyle,
    Writable,
    WriteBehavior,
    EditBehavior,
    ScrollBehavior,
};
use unsegen::widgets::{
    LogViewer,
    PromptLine,
    Pager,
    FileLineStorage,
    SyntectHighLighter,
};
use input::{
    InputEvent,
};
use syntect::highlighting::{
    Theme,
};
use syntect::parsing::{
    SyntaxSet,
};
use std::io;
use std::path::Path;
use gdbmi::output::{
    OutOfBandRecord,
    AsyncKind,
    AsyncClass,
    NamedValues,
};

struct Console {
    text_area: LogViewer,
    prompt_line: PromptLine,
    layout: VerticalLayout,
}

impl Console {
    pub fn new() -> Self {
        Console {
            text_area: LogViewer::new(),
            prompt_line: PromptLine::with_prompt("(gdb) ".into()),
            layout: VerticalLayout::new(unsegen::SeparatingStyle::Draw('=')),
        }
    }

    pub fn add_message(&mut self, msg: String) {
        use std::fmt::Write;
        write!(self.text_area, " -=- {}\n", msg).expect("Write message");
    }

    pub fn event(&mut self, input: unsegen::Input, gdb: &mut gdbmi::GDB) { //TODO more console events
        if input.event == Event::Key(Key::Char('\n')) {
            let line = self.prompt_line.finish_line().to_owned();
            match line.as_ref() {
                "!stop" => {
                    gdb.interrupt_execution().expect("interrupted gdb");

                    // This does not always seem to unblock gdb, but only hang it
                    //use gdbmi::input::MiCommand;
                    //gdb.execute(&MiCommand::exec_interrupt()).expect("Interrupt ");
                },
                // Gdb commands
                _ => {
                    self.add_message(format!("(gdb) {}", line));
                    match gdb.execute(&gdbmi::input::MiCommand::cli_exec(line)) {
                        Ok(result) => {
                            self.add_message(format!("Result: {:?}", result));
                        },
                        Err(gdbmi::ExecuteError::Quit) => { self.add_message(format!("quit")); },
                        Err(gdbmi::ExecuteError::Busy) => { self.add_message(format!("GDB is running!")); },
                        //Err(err) => { panic!("Unknown error {:?}", err) },
                    }
                },
            }
        } else {
            let _ = input.chain(
                    |i: Input| if let (&Event::Key(Key::Ctrl('c')), true) = (&i.event, self.prompt_line.line.get().is_empty()) {
                        gdb.interrupt_execution().expect("interrupted gdb");
                        None
                    } else {
                        Some(i)
                    }
                    )
                .chain(
                    EditBehavior::new(&mut self.prompt_line)
                        .left_on(Key::Left)
                        .right_on(Key::Right)
                        .up_on(Key::Up)
                        .down_on(Key::Down)
                        .delete_symbol_on(Key::Delete)
                        .remove_symbol_on(Key::Backspace)
                        .clear_on(Key::Ctrl('c'))
                    )
                .chain(
                    ScrollBehavior::new(&mut self.text_area)
                        .forwards_on(Key::PageDown)
                        .backwards_on(Key::PageUp)
                    );
        }
    }
}

impl Widget for Console {
    fn space_demand(&self) -> (Demand, Demand) {
        let widgets: Vec<&Widget> = vec![&self.text_area, &self.prompt_line];
        self.layout.space_demand(widgets.as_slice())
    }
    fn draw(&mut self, window: Window) {
        let mut widgets: Vec<&mut Widget> = vec![&mut self.text_area, &mut self.prompt_line];
        self.layout.draw(window, &mut widgets)
    }
}

// Terminal ---------------------------------------------------------------------------------------

use pty;
pub struct PseudoTerminal {
    //width: u32,
    //height: u32,
    pty: pty::PTYInput,
    display: unsegen::widgets::LogViewer,
    //prompt_line: unsegen::widgets::PromptLine,
    //layout: unsegen::VerticalLayout,

    input_buffer: Vec<u8>,
}

impl PseudoTerminal {
    pub fn new(pty: pty::PTYInput) -> Self {
        PseudoTerminal {
            pty: pty,
            display: unsegen::widgets::LogViewer::new(),
            //prompt_line: unsegen::widgets::PromptLine::with_prompt("".into()),
            //layout: unsegen::VerticalLayout::new(unsegen::SeparatingStyle::Draw('=')),
            input_buffer: Vec::new(),
        }
    }

    fn add_byte_input(&mut self, mut bytes: Vec<u8>) {
        self.input_buffer.append(&mut bytes);

        //TODO: handle control sequences?
        if let Ok(string) = String::from_utf8(self.input_buffer.clone()) {
            use std::fmt::Write;
            self.display.write_str(&string).expect("Write byte to terminal");
            self.input_buffer.clear();
        }
    }
}

impl Widget for PseudoTerminal {
    fn space_demand(&self) -> (Demand, Demand) {
        //let widgets: Vec<&unsegen::Widget> = vec![&self.display, &self.prompt_line];
        //self.layout.space_demand(widgets.into_iter())
        return self.display.space_demand();
    }
    fn draw(&mut self, window: Window) {
        //let widgets: Vec<&unsegen::Widget> = vec![&self.display, &self.prompt_line];
        //self.layout.draw(window, &widgets)
        self.display.draw(window);
    }
}

impl Writable for PseudoTerminal {
    fn write(&mut self, c: char) {
        use std::io::Write;
        write!(self.pty, "{}", c).expect("Write key to terminal");
    }
}

// Gui --------------------------------------------------------------------------------
pub struct Gui<'a> {
    console: Console,
    process_pty: PseudoTerminal,
    highlighting_theme: &'a Theme,
    file_viewer: Pager<FileLineStorage, SyntectHighLighter<'a>>,
    syntax_set: SyntaxSet,

    left_layout: VerticalLayout,
    right_layout: VerticalLayout,
}

#[derive(Debug)]
pub enum PagerShowError {
    CouldNotOpenFile(io::Error),
    LineDoesNotExist(usize),
}

impl<'a> Gui<'a> {

    pub fn new(process_pty: ::pty::PTYInput, highlighting_theme: &'a Theme) -> Self {
        Gui {
            console: Console::new(),
            process_pty: PseudoTerminal::new(process_pty),
            highlighting_theme: highlighting_theme,
            file_viewer: Pager::new(),
            syntax_set: SyntaxSet::load_defaults_nonewlines(),
            left_layout: VerticalLayout::new(SeparatingStyle::Draw('=')),
            right_layout: VerticalLayout::new(SeparatingStyle::Draw('=')),
        }
    }

    pub fn show_in_file_viewer<P: AsRef<Path>>(&mut self, path: P, line: usize) -> Result<(), PagerShowError> {
        let need_to_reload = if let Some(ref content) = self.file_viewer.content {
            content.storage.get_file_path() != path.as_ref()
        } else {
            true
        };
        if need_to_reload {
            try!{self.load_in_file_viewer(path).map_err(|e| PagerShowError::CouldNotOpenFile(e))};
        }
        self.file_viewer.go_to_line(line).map_err(|_| PagerShowError::LineDoesNotExist(line))
    }

    pub fn load_in_file_viewer<P: AsRef<Path>>(&mut self, path: P) -> io::Result<()> {
        let file_storage = try!{FileLineStorage::new(path.as_ref())};
        let syntax = self.syntax_set.find_syntax_for_file(path.as_ref())
            .expect("file IS openable, see file storage")
            .unwrap_or(self.syntax_set.find_syntax_plain_text());
        self.file_viewer.load(file_storage, SyntectHighLighter::new(syntax, self.highlighting_theme));
        Ok(())
    }

    fn handle_async_record(&mut self, kind: AsyncKind, class: AsyncClass, mut results: NamedValues) {
        match (kind, class) {
            (AsyncKind::Exec, AsyncClass::Stopped) => {
                self.console.add_message(format!("stopped: {:?}", results));
                let mut frame = results.remove("frame").expect("frame present").unwrap_tuple_or_named_value_list();
                let path = frame.remove("fullname").expect("fullname present").unwrap_const();
                let line = frame.remove("line").expect("line present").unwrap_const().parse::<usize>().expect("parse usize") - 1; //TODO we probably want to treat the conversion line_number => buffer index somewhere else...
                self.show_in_file_viewer(path, line).expect("gdb surely would never lie to us!");
            },
            (kind, class) => self.console.add_message(format!("unhandled async_record: [{:?}, {:?}] {:?}", kind, class, results)),
        }
    }

    pub fn add_out_of_band_record(&mut self, record: OutOfBandRecord) {
        match record {
            OutOfBandRecord::StreamRecord{ kind: _, data} => {
                use std::fmt::Write;
                write!(self.console.text_area, "{}", data).expect("Write message");
            },
            OutOfBandRecord::AsyncRecord{token: _, kind, class, results} => {
                self.handle_async_record(kind, class, results);
            },

        }
    }

    pub fn add_pty_input(&mut self, input: Vec<u8>) {
        self.process_pty.add_byte_input(input);
    }

    pub fn add_debug_message(&mut self, msg: &str) {
        self.console.add_message(format!("Debug: {}", msg));
    }

    pub fn draw(&mut self, window: Window) {
        use unsegen::{TextAttribute, Color, Style};
        let split_pos = window.get_width()/2-1;
        let (window_l, rest) = window.split_h(split_pos);

        let (mut separator, window_r) = rest.split_h(2);

        separator.set_default_format(TextAttribute::new(Color::green(), Color::blue(), Style::new().bold().italic().underline()));
        separator.fill('|');

        let mut left_widgets: Vec<&mut Widget> = vec![&mut self.console];
        self.left_layout.draw(window_l, &mut left_widgets);

        let mut right_widgets: Vec<&mut Widget> = vec![&mut self.file_viewer, &mut self.process_pty];
        self.right_layout.draw(window_r, &mut right_widgets);
    }

    pub fn event(&mut self, event: ::input::InputEvent, gdb: &mut gdbmi::GDB) { //TODO more console events
        match event {
            InputEvent::ConsoleEvent(event) => {
                self.console.event(event, gdb);
            },
            InputEvent::PseudoTerminalEvent(event) => {
                event.chain(WriteBehavior::new(&mut self.process_pty));
            },
            InputEvent::SourcePagerEvent(event) => {
                event.chain(ScrollBehavior::new(&mut self.file_viewer)
                            .forwards_on(Key::PageDown)
                            .backwards_on(Key::PageUp)
                            );
            },
            InputEvent::Quit => {
                unreachable!("quit should have been caught in main" )
            }, //TODO this is ugly
        }
    }
}
