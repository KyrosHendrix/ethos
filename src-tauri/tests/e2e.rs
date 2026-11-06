//! Drives the real app through WebView2's DevTools protocol.
//!
//! Run with `cargo test -p ethos --features e2e --test e2e` after `trunk build --release`.
//! It uses a temporary profile and generated PDFs, never your normal app profile.
#![cfg(feature = "e2e")]

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread::sleep;
use std::time::{Duration, Instant};

use ethos_core::{Mark, MarkKind};
use ethos_engine::{create_sample, read_fields, split_marks};
use lopdf::encryption::crypt_filters::{Aes128CryptFilter, CryptFilter};
use lopdf::{Document, EncryptionState, EncryptionVersion, Object, Permissions};
use serde_json::{Value, json};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket, connect};

const TIMEOUT: Duration = Duration::from_secs(20);
/// Helpers available to every evaluated expression.
const PRELUDE: &str = r#"
const named = (name) => [...document.querySelectorAll('button,[role=option],[role=tab]')]
  .find((node) => !node.closest('[inert]') && (node.getAttribute('aria-label') ?? node.textContent.trim()) === name);
const sized = (node) => { const box = node.getBoundingClientRect();
  return { x: box.left + box.width / 2, y: box.top + box.height / 2, left: box.left, top: box.top }; };
"#;

fn http_get(port: u16, path: &str) -> Option<Value> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    // The server keeps the socket open, so a short timeout ends the read.
    stream
        .set_read_timeout(Some(Duration::from_millis(300)))
        .ok()?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"
    )
    .ok()?;
    let mut response = Vec::new();
    // The server may reset the connection after replying, so keep whatever arrived.
    let _ = stream.read_to_end(&mut response);
    let response = String::from_utf8_lossy(&response);
    serde_json::from_str(response.split_once("\r\n\r\n")?.1).ok()
}

/// Stops the app however the test ends, including while it is still starting.
struct Process(Child);

impl Drop for Process {
    fn drop(&mut self) {
        // WebView2 child processes would otherwise keep the debugging port open.
        let tree = ["/F", "/T", "/PID", &self.0.id().to_string()];
        let _ = Command::new("taskkill").args(tree).output();
        let _ = self.0.wait();
    }
}

struct App {
    _process: Process,
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    next_id: u64,
    errors: Vec<String>,
    profile: PathBuf,
    shots: PathBuf,
}

impl App {
    fn launch(folder: &Path, port: u16, args: &[&Path]) -> Self {
        let profile = folder.join("profile");
        fs::create_dir_all(&profile).unwrap();
        let process = Command::new(env!("CARGO_BIN_EXE_ethos"))
            .args(args)
            .env("ETHOS_USER_DATA", &profile)
            .env("WEBVIEW2_USER_DATA_FOLDER", folder.join("webview"))
            .env(
                "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
                format!("--remote-debugging-port={port}"),
            )
            .spawn()
            .map(Process)
            .expect("the app starts");
        // A new WebView2 profile can take a while to create.
        let deadline = Instant::now() + 3 * TIMEOUT;
        let socket_url = loop {
            let target = http_get(port, "/json/list").and_then(|list| {
                list.as_array()?
                    .iter()
                    .find(|target| target["type"] == "page")?["webSocketDebuggerUrl"]
                    .as_str()
                    .map(str::to_owned)
            });
            if let Some(url) = target {
                break url;
            }
            assert!(Instant::now() < deadline, "the app exposed no page");
            sleep(Duration::from_millis(100));
        };
        let (socket, _) = connect(socket_url).expect("DevTools connects");
        if let MaybeTlsStream::Plain(stream) = socket.get_ref() {
            stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        }
        let shots = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("e2e");
        fs::create_dir_all(&shots).unwrap();
        let mut app = Self {
            _process: process,
            socket,
            next_id: 0,
            errors: Vec::new(),
            profile,
            shots,
        };
        app.send("Runtime.enable", json!({}));
        app.send("Page.enable", json!({}));
        app
    }

    fn send(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        let request = json!({ "id": id, "method": method, "params": params });
        self.socket
            .send(Message::text(request.to_string()))
            .unwrap();
        loop {
            let message = self.socket.read().expect("DevTools stays connected");
            let Ok(text) = message.to_text() else {
                continue;
            };
            let Ok(reply) = serde_json::from_str::<Value>(text) else {
                continue;
            };
            if reply["method"] == "Runtime.consoleAPICalled" && reply["params"]["type"] == "error" {
                self.errors.push(reply["params"]["args"].to_string());
            }
            if reply["method"] == "Runtime.exceptionThrown" {
                self.errors
                    .push(reply["params"]["exceptionDetails"].to_string());
            }
            if reply["id"] == id {
                assert!(reply["error"].is_null(), "{method} failed: {reply}");
                return reply["result"].clone();
            }
        }
    }

    fn eval(&mut self, expression: &str) -> Value {
        let params = json!({
            "expression": format!("(() => {{ {PRELUDE}\n return ({expression}); }})()"),
            "returnByValue": true,
        });
        let reply = self.send("Runtime.evaluate", params);
        assert!(
            reply["exceptionDetails"].is_null(),
            "{expression} threw {}",
            reply["exceptionDetails"]
        );
        reply["result"]["value"].clone()
    }

    fn wait(&mut self, expression: &str) {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            match self.eval(expression) {
                Value::Bool(false) | Value::Null => {}
                _ => return,
            }
            if Instant::now() > deadline {
                self.shot("failure");
                panic!("timed out waiting for {expression}");
            }
            sleep(Duration::from_millis(40));
        }
    }

    fn mouse(&mut self, kind: &str, x: f64, y: f64, buttons: u32) {
        let button = if kind == "mouseMoved" && buttons == 0 {
            "none"
        } else {
            "left"
        };
        let params = json!({ "type": kind, "x": x, "y": y, "button": button, "buttons": buttons, "clickCount": 1 });
        self.send("Input.dispatchMouseEvent", params);
    }

    fn click_at(&mut self, x: f64, y: f64) {
        self.mouse("mouseMoved", x, y, 0);
        self.mouse("mousePressed", x, y, 1);
        self.mouse("mouseReleased", x, y, 0);
    }

    fn center(&mut self, expression: &str) -> (f64, f64) {
        let center = self.eval(&format!("sized({expression})"));
        (center["x"].as_f64().unwrap(), center["y"].as_f64().unwrap())
    }

    fn click(&mut self, name: &str) {
        self.wait(&format!("named({name:?})"));
        let (x, y) = self.center(&format!("named({name:?})"));
        self.click_at(x, y);
    }

    fn press(&mut self, key: &str, code: u32, modifiers: u32) {
        let text = if modifiers == 0 && key.len() == 1 {
            key
        } else {
            ""
        };
        let text = if key == "Enter" { "\r" } else { text };
        self.send(
            "Input.dispatchKeyEvent",
            json!({ "type": "keyDown", "key": key, "windowsVirtualKeyCode": code, "modifiers": modifiers, "text": text }),
        );
        self.send(
            "Input.dispatchKeyEvent",
            json!({ "type": "keyUp", "key": key, "windowsVirtualKeyCode": code, "modifiers": modifiers }),
        );
    }

    fn control(&mut self, key: char) {
        self.press(&key.to_string(), key.to_ascii_uppercase() as u32, CONTROL);
    }

    fn fill(&mut self, selector: &str, text: &str) {
        self.wait(&format!("document.querySelector({selector:?})"));
        self.eval(&format!("(document.querySelector({selector:?}).focus(), document.querySelector({selector:?}).select(), true)"));
        self.send("Input.insertText", json!({ "text": text }));
    }

    fn count(&mut self, selector: &str) -> u64 {
        self.eval(&format!("document.querySelectorAll({selector:?}).length"))
            .as_u64()
            .unwrap()
    }

    fn wait_count(&mut self, selector: &str, count: u64) {
        self.wait(&format!(
            "document.querySelectorAll({selector:?}).length === {count}"
        ));
    }

    fn wait_text(&mut self, text: &str) {
        self.wait(&format!("document.body.innerText.includes({text:?})"));
    }

    fn rendered(&mut self) {
        self.wait(
            "[...document.querySelectorAll('.page-canvas')].some((canvas) => canvas.width > 100) \
             && document.querySelector('.textLayer')?.textContent.length > 30",
        );
    }

    fn shot(&mut self, name: &str) {
        let reply = self.send("Page.captureScreenshot", json!({ "format": "png" }));
        let png = decode_base64(reply["data"].as_str().unwrap_or_default());
        fs::write(self.shots.join(format!("{name}.png")), png).unwrap();
    }

    fn script_dialog(&self, file: &str, answer: &Path) {
        fs::write(self.profile.join(file), answer.to_string_lossy().as_bytes()).unwrap();
    }

    fn finish(self) {
        assert_eq!(self.errors, Vec::<String>::new(), "uncaught page errors");
    }
}

const CONTROL: u32 = 2;
const CONTROL_SHIFT: u32 = 10;

fn decode_base64(text: &str) -> Vec<u8> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = Vec::with_capacity(text.len() * 3 / 4);
    let (mut buffer, mut bits) = (0_u32, 0);
    for byte in text.bytes().filter(|byte| *byte != b'=') {
        let value = ALPHABET.iter().position(|item| *item == byte).unwrap_or(0) as u32;
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    output
}

/// A unique folder removed on drop, once the app has released it.
struct Folder(PathBuf);

impl Folder {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("ethos-e2e-{name}-{}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        // WebView2 can hold files briefly after exit; the OS temp cleanup gets stragglers.
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn protected_pdf() -> Vec<u8> {
    let mut document = Document::load_mem(&create_sample().unwrap()).unwrap();
    let id = Object::string_literal("ethos-e2e-id");
    document.trailer.set("ID", vec![id.clone(), id]);
    let filter: std::sync::Arc<dyn CryptFilter> = std::sync::Arc::new(Aes128CryptFilter);
    let version = EncryptionVersion::V4 {
        document: &document,
        encrypt_metadata: true,
        crypt_filters: std::collections::BTreeMap::from([(b"StdCF".to_vec(), filter)]),
        stream_filter: b"StdCF".to_vec(),
        string_filter: b"StdCF".to_vec(),
        owner_password: "owner",
        user_password: "secret",
        permissions: Permissions::all(),
    };
    let state = EncryptionState::try_from(version).unwrap();
    document.encrypt(&state).unwrap();
    let mut bytes = Vec::new();
    document.save_to(&mut bytes).unwrap();
    bytes
}

/// Passes through the interface workflows in one session, as a person would.
#[test]
fn reader_workflows() {
    let folder = Folder::new("reader");
    let fixture = folder.0.join("Smoke fixture.pdf");
    let output = folder.0.join("export.pdf");
    let original = create_sample().unwrap();
    fs::write(&fixture, &original).unwrap();
    let mut app = App::launch(&folder.0, 9347, &[]);

    println!("welcome and sidebar");
    app.wait("named('Open sample')");
    assert_eq!(app.count(".titlebar img, .titlebar .brand"), 0);
    assert_eq!(
        app.eval("named('Toggle sidebar').getAttribute('aria-expanded')"),
        "true"
    );
    app.click("Toggle sidebar");
    app.wait("document.querySelector('.library-sidebar').getAttribute('inert') === ''");
    app.wait(
        "Math.abs(document.querySelector('.sidebar-shell').getBoundingClientRect().width - 44) < 1",
    );
    assert_eq!(
        app.eval("named('Toggle sidebar').getAttribute('aria-expanded')"),
        "false"
    );
    app.click("Toggle sidebar");
    app.wait(
        "(() => { const shell = document.querySelector('.sidebar-shell'); return Math.abs(shell.getBoundingClientRect().width \
         - parseFloat(getComputedStyle(shell).getPropertyValue('--sidebar-width'))) < 1; })()",
    );
    assert_eq!(
        app.eval("getComputedStyle(document.querySelector('.library-actions .primary-button')).borderRadius"),
        "5px"
    );
    app.shot("welcome");

    println!("tooltip appears after the hover delay");
    let (x, y) = app.center("named('Toggle sidebar')");
    app.mouse("mouseMoved", 700.0, 500.0, 0);
    app.mouse("mouseMoved", x, y, 0);
    app.wait("document.querySelector('.ui-tooltip')?.textContent === 'Toggle sidebar'");
    app.wait("document.querySelector('.ui-tooltip').style.left !== ''");
    app.shot("tooltip");
    app.mouse("mouseMoved", 700.0, 500.0, 0);
    app.wait_count(".ui-tooltip", 0);

    println!("sample renders a canvas and selectable text");
    app.click("Open sample");
    app.rendered();
    let selected = app.eval(
        "(() => { const range = document.createRange(); range.selectNodeContents(document.querySelector('.textLayer')); \
         const selection = getSelection(); selection.removeAllRanges(); selection.addRange(range); \
         const text = selection.toString(); selection.removeAllRanges(); return text; })()",
    );
    assert!(selected.as_str().unwrap().contains("ethos"), "{selected}");
    app.eval(
        "(() => { const range = document.createRange(); range.selectNodeContents(document.querySelector('.textLayer'));          getSelection().addRange(range); return true; })()",
    );
    app.shot("selection");
    app.eval("(getSelection().removeAllRanges(), true)");
    let inked = app.eval(
        "(() => { const canvas = document.querySelector('.page-canvas'); \
         const data = canvas.getContext('2d').getImageData(0, 0, canvas.width, canvas.height).data; \
         for (let i = 0; i < data.length; i += 4) if (data[i] < 100) return true; return false; })()",
    );
    assert_eq!(inked, true, "the canvas holds rendered page content");
    app.wait("document.querySelector('.thumbnail canvas')?.width > 50");
    app.shot("reader");

    println!("search shortcut finds page text and marks it");
    app.control('f');
    app.fill("[aria-label='Search document']", "attention");
    app.wait("document.querySelector('.search-result')");
    app.wait_text("match");
    app.wait("document.querySelector('.textLayer .search-hit')");

    println!("page jump and zoom controls");
    app.fill("[aria-label='Page number']", "3");
    app.press("Enter", 13, 0);
    app.wait("document.querySelector('[aria-label=\"Page number\"]').value === '3'");
    app.click("Zoom level");
    app.wait("document.activeElement?.getAttribute('role') === 'listbox'");
    app.shot("zoom-menu");
    app.click("100%");
    app.wait("named('Zoom level') && document.querySelector('[aria-label=\"Zoom level\"]').innerText === '100%'");
    assert_eq!(app.count("[role=listbox]"), 0);
    app.click("Zoom in");
    app.wait("parseInt(document.querySelector('[aria-label=\"Zoom level\"]').innerText) > 100");
    app.click("Zoom level");
    app.wait("document.activeElement?.getAttribute('role') === 'listbox'");
    app.press("Escape", 27, 0);
    app.wait_count("[role=listbox]", 0);
    app.click("Zoom level");
    app.wait("document.activeElement?.getAttribute('role') === 'listbox'");
    app.click("Fit width");
    app.wait("document.querySelector('[aria-label=\"Zoom level\"]').innerText === 'Fit width'");
    app.click("Pages");
    app.fill("[aria-label='Page number']", "1");
    app.press("Enter", 13, 0);

    println!("highlight selected text");
    app.click("Highlight (H)");
    app.wait("[...document.querySelectorAll('.pdf-page[data-page=\"1\"] .textLayer span')].some((span) => span.textContent.includes('The best tools give you space.'))");
    app.eval(
        "(() => { const span = [...document.querySelectorAll('.pdf-page[data-page=\"1\"] .textLayer span')]\
         .find((node) => node.textContent.includes('The best tools give you space.')); \
         const range = document.createRange(); range.selectNodeContents(span); \
         const selection = getSelection(); selection.removeAllRanges(); selection.addRange(range); \
         span.dispatchEvent(new MouseEvent('mouseup', { bubbles: true })); return true; })()",
    );
    app.wait("document.querySelectorAll('.annotation-layer rect').length > 0");
    app.click("Undo (Ctrl Z)");
    app.wait_count(".annotation-layer rect", 0);

    println!("draw a stroke and undo it");
    app.click("Draw (D)");
    app.wait("document.querySelector('.pdf-page[data-page=\"1\"] .drawing-layer')");
    let bounds = app.eval("sized(document.querySelector('.pdf-page'))");
    let (left, top) = (
        bounds["left"].as_f64().unwrap(),
        bounds["top"].as_f64().unwrap(),
    );
    app.mouse("mouseMoved", left + 100.0, top + 180.0, 0);
    app.mouse("mousePressed", left + 100.0, top + 180.0, 1);
    for step in 1..=10 {
        let step = f64::from(step);
        app.mouse(
            "mouseMoved",
            left + 100.0 + 7.0 * step,
            top + 180.0 + 2.0 * step,
            1,
        );
    }
    app.mouse("mouseReleased", left + 170.0, top + 200.0, 0);
    app.wait_count(".annotation-layer polyline", 1);
    app.click("Undo (Ctrl Z)");
    app.wait_count(".annotation-layer polyline", 0);

    println!("text note through the page and modal");
    let (x, y) = app.center("document.querySelector('.document-heading')");
    app.click_at(x, y);
    app.press("t", 84, 0);
    app.wait("document.querySelector('.pdf-page.tool-text .drawing-layer')");
    app.click_at(left + 150.0, top + 220.0);
    app.wait("document.activeElement?.tagName === 'TEXTAREA'");
    app.shot("note-dialog");
    app.fill("[aria-label='Text note']", "Smoke annotation");
    app.eval("[...document.querySelectorAll('[role=dialog] button')].find((node) => node.textContent === 'Add text').click()");
    app.wait_count("[role=dialog]", 0);
    app.wait_count(".annotation-layer text", 1);
    let (x, y) = app.center("document.querySelector('#document-tab-notes')");
    app.click_at(x, y);
    app.wait_count(".note-item", 1);
    app.wait_text("Smoke annotation");
    assert_eq!(
        app.eval("named('Toggle sidebar') !== null && !!document.querySelector('.dirty-dot')"),
        true
    );

    println!("undo and redo shortcuts and removing a note");
    let (x, y) = app.center("document.querySelector('.document-heading')");
    app.click_at(x, y);
    app.control('z');
    app.wait_count(".note-item", 0);
    app.press("z", 90, CONTROL_SHIFT);
    app.wait_count(".note-item", 1);
    app.click("Remove annotation");
    app.wait_count(".note-item", 0);
    app.click("Undo (Ctrl Z)");
    app.wait_count(".note-item", 1);

    println!("leaving with unsaved edits asks first and keeps them by default");
    app.click("Library");
    app.wait("document.querySelector('[role=dialog]')?.innerText.includes('Keep your changes?')");
    app.press("Escape", 27, 0);
    app.wait_count("[role=dialog]", 0);
    assert_eq!(app.count(".note-item"), 1);
    app.click("Close window");
    app.wait("document.querySelector('[role=dialog]')?.innerText.includes('Keep your changes?')");
    app.shot("discard-dialog");
    assert_eq!(
        app.eval("document.activeElement === named('Keep editing')"),
        true,
        "the safe choice has focus"
    );
    app.click("Keep editing");
    app.wait_count("[role=dialog]", 0);
    assert_eq!(app.count(".note-item"), 1);

    println!("rotate, delete and undo pages");
    app.click("Pages");
    app.click("Rotate current page");
    app.wait("document.querySelector('.pdf-page[data-page=\"1\"]').offsetWidth > document.querySelector('.pdf-page[data-page=\"1\"]').offsetHeight");
    app.rendered();
    app.shot("rotated");
    app.click("Undo (Ctrl Z)");
    app.click("Redo (Ctrl Shift Z)");
    app.wait("document.querySelector('.pdf-page[data-page=\"1\"]').offsetWidth > document.querySelector('.pdf-page[data-page=\"1\"]').offsetHeight");
    app.click("Delete current page");
    app.wait_count(".thumbnail", 3);
    app.wait_text("Page removed.");
    app.click("Undo");
    app.wait_count(".thumbnail", 4);
    assert_eq!(app.eval("window.ethos === undefined"), true);

    println!("save dialog writes a readable annotated rotated PDF");
    app.script_dialog("save-dialog.txt", &output);
    app.control('s');
    app.wait_text("Copy saved. Your original is untouched.");
    assert_eq!(
        fs::read_to_string(app.profile.join("save-suggestion.txt")).unwrap(),
        "A little more room to think - edited.pdf"
    );
    let saved_bytes = fs::read(&output).unwrap();
    let saved = Document::load_mem(&saved_bytes).unwrap();
    assert_eq!(saved.get_pages().len(), 4);
    let first = saved.get_dictionary(saved.get_pages()[&1]).unwrap();
    assert_eq!(first.get(b"Rotate").unwrap().as_i64().unwrap(), 90);
    let (_, marks) = split_marks(&saved_bytes);
    assert!(
        matches!(&marks[..], [Mark { kind: MarkKind::Text { text, .. }, .. }] if text == "Smoke annotation"),
        "{marks:?}"
    );
    app.wait("!document.querySelector('.dirty-dot')");

    println!("open dialog loads a PDF and recents reopen it");
    app.script_dialog("open-dialog.txt", &fixture);
    app.control('o');
    app.wait("document.querySelector('.document-heading')?.innerText.trim() === 'Smoke fixture'");
    app.rendered();
    assert_eq!(fs::read(&fixture).unwrap(), original);
    app.click("Library");
    app.wait("document.querySelector('.recent-file')");
    app.shot("library-recents");
    app.click("Smoke fixture.pdf");
    app.wait("document.querySelector('.document-heading')?.innerText.trim() === 'Smoke fixture'");
    app.rendered();

    println!("cancelled dialogs change nothing");
    app.script_dialog("open-dialog.txt", Path::new(""));
    app.control('o');
    sleep(Duration::from_millis(300));
    assert_eq!(
        app.eval("document.querySelector('.document-heading').innerText.trim()"),
        "Smoke fixture"
    );
    app.script_dialog("save-dialog.txt", Path::new(""));
    let before = fs::read(&output).unwrap();
    app.control('s');
    sleep(Duration::from_millis(300));
    assert_eq!(fs::read(&output).unwrap(), before);

    println!("saving never replaces an original");
    app.script_dialog("save-dialog.txt", &fixture);
    app.control('s');
    app.wait_text("preserve your original");
    assert_eq!(fs::read(&fixture).unwrap(), original);
    app.click("Dismiss message");
    app.script_dialog("save-dialog.txt", &output);
    app.control('s');
    app.wait_text("Copy saved.");

    println!("dark appearance persists across reloads");
    app.click("Use dark appearance");
    app.wait("document.documentElement?.dataset.theme === 'dark'");
    app.eval("(location.reload(), true)");
    sleep(Duration::from_millis(800));
    app.wait("named('Open sample')");
    assert_eq!(app.eval("document.documentElement.dataset.theme"), "dark");
    app.shot("welcome-dark");
    app.click("Open sample");
    app.rendered();
    app.send(
        "Emulation.setDeviceMetricsOverride",
        json!({ "width": 1100, "height": 800, "deviceScaleFactor": 0, "mobile": false }),
    );
    sleep(Duration::from_millis(300));
    app.shot("reader-1100-dark");
    assert_eq!(app.eval("document.body.scrollWidth <= innerWidth"), true);
    app.send("Emulation.clearDeviceMetricsOverride", json!({}));

    println!("discarding needs an explicit choice");
    app.click("Rotate current page");
    app.click("Library");
    app.wait("document.querySelector('[role=dialog]')");
    app.click("Discard changes");
    app.wait("named('Open sample')");
    assert_eq!(app.count("[role=dialog]"), 0);

    println!("shortcuts dialog");
    app.press("?", 191, 0);
    app.wait(
        "document.querySelector('[role=dialog]')?.innerText.includes('A few useful shortcuts')",
    );
    app.shot("shortcuts");
    app.press("Escape", 27, 0);
    app.wait_count("[role=dialog]", 0);

    app.finish();
}

#[test]
fn sidebar_icons_align_with_toggle() {
    let folder = Folder::new("sidebar-alignment");
    let mut app = App::launch(&folder.0, 9356, &[]);
    app.wait("named('Open sample')");
    for width in [1100, 1440] {
        app.send(
            "Emulation.setDeviceMetricsOverride",
            json!({ "width": width, "height": 900, "deviceScaleFactor": 1, "mobile": false }),
        );
        for expanded in [true, false] {
            if app.eval("named('Toggle sidebar').getAttribute('aria-expanded') === 'true'")
                != expanded
            {
                app.click("Toggle sidebar");
            }
            let selector = if expanded {
                ".library-sidebar > .nav-item > svg"
            } else {
                ".library-shell .sidebar-rail svg"
            };
            app.wait_count(selector, if expanded { 3 } else { 2 });
            if expanded {
                app.wait("Math.abs(document.querySelector('.library-sidebar').getBoundingClientRect().left) < 0.1");
            }
            let centers = app.eval(&format!(
                "[...document.querySelectorAll({selector:?})].map(node => sized(node).x)"
            ));
            let toggle = app.eval("sized(named('Toggle sidebar').querySelector('svg')).x");
            for center in centers.as_array().unwrap() {
                assert!(
                    (center.as_f64().unwrap() - toggle.as_f64().unwrap()).abs() < 0.5,
                    "sidebar icons must align at width {width}, expanded={expanded}: {centers} vs {toggle}"
                );
            }
        }
    }
    app.finish();
}

/// Tauri's drag script swallows presses it turns into window drags, so a press that reaches
/// `window` was left to the page. Uses synthetic input so the run never moves the real mouse.
#[test]
fn title_bar_starts_a_window_drag() {
    let folder = Folder::new("titlebar");
    let mut app = App::launch(&folder.0, 9351, &[]);
    app.wait("named('Open sample')");
    app.click("Open sample");
    app.rendered();
    app.eval(
        "(window.__pressed = 0, addEventListener('mousedown', () => window.__pressed++), true)",
    );
    let header = app.eval("sized(document.querySelector('.titlebar'))");
    let (x, y) = (
        header["left"].as_f64().unwrap() + 400.0,
        header["y"].as_f64().unwrap(),
    );
    let hit = format!("document.elementFromPoint({x}, {y}).classList.contains('titlebar')");
    assert_eq!(app.eval(&hit), true);
    app.click_at(x, y);
    sleep(Duration::from_millis(200));
    assert_eq!(
        app.eval("window.__pressed"),
        0,
        "the empty title bar drags the window"
    );

    let (x, y) = app.center("document.querySelector('.document-heading')");
    app.click_at(x, y);
    app.wait("window.__pressed === 1");
    app.click("Toggle sidebar");
    app.wait("window.__pressed === 2");
    app.finish();
}

/// A protected PDF passed on the command line asks for its password, then opens.
#[test]
fn protected_file_from_the_command_line() {
    let folder = Folder::new("protected");
    let file = folder.0.join("Protected.pdf");
    fs::write(&file, protected_pdf()).unwrap();
    let mut app = App::launch(&folder.0, 9348, &[file.as_path()]);
    app.wait("document.querySelector('[role=dialog]')?.innerText.includes('A little privacy')");
    app.shot("password-dialog");
    app.fill("[aria-label='PDF password']", "wrong");
    app.press("Enter", 13, 0);
    app.wait_text("That password didn’t work. Try again.");
    app.fill("[aria-label='PDF password']", "secret");
    app.press("Enter", 13, 0);
    app.wait("document.querySelector('.document-heading')?.innerText.trim() === 'Protected'");
    app.rendered();
    app.script_dialog("save-dialog.txt", &folder.0.join("copy.pdf"));
    app.control('s');
    app.wait_text("Protected PDFs can be read here");
    app.finish();
}

/// Dropped files go through the browser's real drag pipeline and keep their paths.
#[test]
fn dropped_file_opens() {
    let folder = Folder::new("drop");
    let file = folder.0.join("Dropped.pdf");
    fs::write(&file, create_sample().unwrap()).unwrap();
    let mut app = App::launch(&folder.0, 9349, &[]);
    app.wait("named('Open sample')");
    let path = file.to_string_lossy().into_owned();
    let drag = |kind: &str, data: Value| json!({ "type": kind, "x": 700, "y": 500, "data": data });
    let files = json!({ "items": [], "files": [path], "dragOperationsMask": 1 });
    app.send("Input.dispatchDragEvent", drag("dragEnter", files.clone()));
    app.send("Input.dispatchDragEvent", drag("dragOver", files.clone()));
    app.wait_text("Drop your PDF to open it in ethos.");
    app.shot("drop-overlay");
    app.send("Input.dispatchDragEvent", drag("drop", files));
    app.wait_count(".drop-overlay", 0);
    app.wait("document.querySelector('.document-heading')?.innerText.trim() === 'Dropped'");
    app.rendered();
    app.click("Library");
    app.wait("named('Dropped.pdf')");
    let text = json!({ "items": [{ "mimeType": "text/plain", "data": "words" }], "dragOperationsMask": 1 });
    app.send("Input.dispatchDragEvent", drag("dragEnter", text.clone()));
    app.send("Input.dispatchDragEvent", drag("dragOver", text));
    sleep(Duration::from_millis(200));
    assert_eq!(app.count(".drop-overlay"), 0, "dragged text is not a file");

    // Reloading must reinstall the bridge, and a second drop must keep every file.
    app.send("Page.reload", json!({}));
    app.wait("named('Open sample')");
    let second = folder.0.join("Another.pdf");
    fs::write(&second, create_sample().unwrap()).unwrap();
    let files = json!({ "items": [], "files": [file, second], "dragOperationsMask": 1 });
    app.send("Input.dispatchDragEvent", drag("dragEnter", files.clone()));
    app.send("Input.dispatchDragEvent", drag("dragOver", files.clone()));
    app.send("Input.dispatchDragEvent", drag("drop", files));
    app.wait("document.querySelector('[role=dialog]')?.innerText.includes('Combine PDFs')");
    app.wait_count(".file-row", 2);
    app.wait_text("Dropped.pdf");
    app.wait_text("Another.pdf");
    app.finish();
}

/// Places a note on the first page through the real interface.
fn add_note(app: &mut App, text: &str) {
    app.rendered();
    let (x, y) = app.center("document.querySelector('.document-heading')");
    app.click_at(x, y);
    app.press("t", 84, 0);
    app.wait("document.querySelector('.pdf-page[data-page=\"1\"].tool-text .drawing-layer')");
    let bounds = app.eval("sized(document.querySelector('.pdf-page[data-page=\"1\"]'))");
    let (left, top) = (
        bounds["left"].as_f64().unwrap(),
        bounds["top"].as_f64().unwrap(),
    );
    app.click_at(left + 150.0, top + 220.0);
    app.wait("document.activeElement?.tagName === 'TEXTAREA'");
    app.fill("[aria-label='Text note']", text);
    app.eval("[...document.querySelectorAll('[role=dialog] button')].find((node) => node.textContent === 'Add text').click()");
    app.wait_count("[role=dialog]", 0);
}

/// Marks survive closing without saving, restarting, and a trip through a saved copy.
#[test]
fn marks_last_without_saving() {
    let folder = Folder::new("marks");
    let file = folder.0.join("Notes.pdf");
    let copy = folder.0.join("Copy.pdf");
    fs::write(&file, create_sample().unwrap()).unwrap();
    let original = fs::read(&file).unwrap();
    {
        let mut app = App::launch(&folder.0, 9352, &[file.as_path()]);
        add_note(&mut app, "Kept for later");
        app.wait_count(".annotation-layer text", 1);
        app.wait("!document.querySelector('.dirty-dot')");

        println!("leaving asks nothing and loses nothing");
        app.click("Library");
        app.wait("named('Notes.pdf')");
        assert_eq!(app.count("[role=dialog]"), 0);
        app.click("Notes.pdf");
        app.wait_text("Your marks are back.");
        app.wait_count(".annotation-layer text", 1);
        app.finish();
    }
    assert_eq!(
        fs::read(&file).unwrap(),
        original,
        "the original never changes"
    );

    println!("marks survive a restart");
    let mut app = App::launch(&folder.0, 9353, &[file.as_path()]);
    app.wait_text("Your marks are back.");
    app.wait_count(".annotation-layer text", 1);

    println!("a saved copy reopens with marks that can still be edited");
    app.script_dialog("save-dialog.txt", &copy);
    app.control('s');
    app.wait_text("Copy saved.");
    app.script_dialog("open-dialog.txt", &copy);
    app.control('o');
    app.wait("document.querySelector('.document-heading')?.innerText.trim() === 'Copy'");
    app.rendered();
    app.wait_count(".annotation-layer text", 1);
    let (x, y) = app.center("document.querySelector('#document-tab-notes')");
    app.click_at(x, y);
    app.wait_count(".note-item", 1);
    app.click("Remove annotation");
    app.wait_count(".annotation-layer text", 0);

    println!("undo brings back what a restored session changed");
    app.click("Library");
    app.wait("named('Notes.pdf')");
    app.click("Notes.pdf");
    app.wait_text("Your marks are back.");
    app.wait_count(".annotation-layer text", 1);
    app.click("Undo");
    app.wait_count(".annotation-layer text", 0);
    app.finish();
}

/// If edits cannot reach disk, leaving still asks first.
#[test]
fn failed_autosave_still_asks_before_leaving() {
    let folder = Folder::new("blocked");
    let file = folder.0.join("Blocked.pdf");
    fs::write(&file, create_sample().unwrap()).unwrap();
    let profile = folder.0.join("profile");
    fs::create_dir_all(&profile).unwrap();
    fs::write(profile.join("marks"), "a file where the folder should go").unwrap();
    let mut app = App::launch(&folder.0, 9354, &[file.as_path()]);
    add_note(&mut app, "At risk");
    app.wait("document.querySelector('.dirty-dot')");
    sleep(Duration::from_millis(500));
    app.wait("document.querySelector('.dirty-dot')");
    app.click("Library");
    app.wait("document.querySelector('[role=dialog]')?.innerText.includes('Keep your changes?')");
    app.click("Keep editing");
    app.wait_count("[role=dialog]", 0);
    assert_eq!(app.count(".annotation-layer text"), 1);
    app.finish();
}

/// Finds and runs a tool the way someone would: Ctrl K, a few letters, Enter.
fn run_from_palette(app: &mut App, query: &str) {
    app.control('k');
    app.wait("document.activeElement?.getAttribute('aria-label') === 'Find a tool'");
    app.send("Input.insertText", json!({ "text": query }));
    app.wait("document.querySelector('.tool-row[data-highlighted]')");
    app.press("Enter", 13, 0);
    app.wait_count(".palette", 0);
}

fn drag(app: &mut App, from: (f64, f64), to: (f64, f64)) {
    app.mouse("mouseMoved", from.0, from.1, 0);
    app.mouse("mousePressed", from.0, from.1, 1);
    for step in 1..=12 {
        let step = f64::from(step) / 12.0;
        app.mouse(
            "mouseMoved",
            from.0 + (to.0 - from.0) * step,
            from.1 + (to.1 - from.1) * step,
            1,
        );
    }
    app.mouse("mouseReleased", to.0, to.1, 0);
}

fn dismiss_toast(app: &mut App) {
    app.wait("document.querySelector('.toast')");
    app.click("Dismiss message");
    app.wait_count(".toast", 0);
}

fn form_pdf() -> Vec<u8> {
    use lopdf::{Dictionary, Stream, dictionary};
    let mut document = Document::load_mem(&create_sample().unwrap()).unwrap();
    let first = document.get_pages()[&1];
    let on = document.add_object(Stream::new(Dictionary::new(), b"q Q\n".to_vec()));
    let off = document.add_object(Stream::new(Dictionary::new(), b"q Q\n".to_vec()));
    let rect = |x: i64, y: i64, w: i64, h: i64| -> Vec<Object> {
        vec![x.into(), y.into(), (x + w).into(), (y + h).into()]
    };
    let text = document.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx",
        "T" => Object::string_literal("full name"), "Rect" => rect(56, 280, 240, 26), "P" => first,
    });
    let check = document.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "FT" => "Btn",
        "T" => Object::string_literal("agree"), "V" => "Off", "AS" => "Off",
        "Rect" => rect(56, 240, 18, 18), "P" => first,
        "AP" => dictionary! { "N" => dictionary! { "Yes" => on, "Off" => off } },
    });
    document
        .get_object_mut(first)
        .and_then(Object::as_dict_mut)
        .unwrap()
        .set("Annots", vec![text.into(), check.into()]);
    let form = document.add_object(dictionary! { "Fields" => vec![text.into(), check.into()] });
    document.catalog_mut().unwrap().set("AcroForm", form);
    let mut bytes = Vec::new();
    document.save_to(&mut bytes).unwrap();
    bytes
}

fn pdf_text(bytes: &[u8]) -> String {
    let document = Document::load_mem(bytes).unwrap();
    let mut text = Vec::new();
    for number in document.get_pages().keys() {
        text.push(document.extract_text(&[*number]).unwrap_or_default());
    }
    text.join(" ")
}

/// Every tool, through the interface, checking the real file it produces.
#[test]
fn tools_produce_real_files() {
    let folder = Folder::new("tools");
    let source = folder.0.join("Report.pdf");
    fs::write(&source, create_sample().unwrap()).unwrap();
    let out = |name: &str| folder.0.join(name);
    let parts = folder.0.join("parts");
    fs::create_dir_all(&parts).unwrap();
    let mut app = App::launch(&folder.0, 9355, &[source.as_path()]);
    app.rendered();
    let script_folder = |app: &App, path: &Path| {
        fs::write(
            app.profile.join("folder-dialog.txt"),
            path.to_string_lossy().as_bytes(),
        )
        .unwrap();
    };

    println!("the tools list opens from the toolbar and filters as you type");
    app.click("Tools (Ctrl K)");
    app.wait("document.activeElement?.getAttribute('aria-label') === 'Find a tool'");
    sleep(Duration::from_millis(250));
    app.shot("tools-list");
    assert!(app.count(".tool-row") >= 17);
    app.send("Input.insertText", json!({ "text": "merge" }));
    app.wait("document.querySelectorAll('.tool-row').length === 1");
    assert_eq!(
        app.eval("document.querySelector('.tool-row').textContent.trim()"),
        "Combine PDFs"
    );
    app.press("Escape", 27, 0);
    app.wait_count(".palette", 0);

    println!("3 save as plain text, 2 Markdown, 10 Word");
    app.script_dialog("save-dialog.txt", &out("Report.txt"));
    run_from_palette(&mut app, "plain text");
    app.wait_text("Saved Report.txt.");
    sleep(Duration::from_millis(300));
    app.shot("tool-done");
    dismiss_toast(&mut app);
    assert!(
        fs::read_to_string(out("Report.txt"))
            .unwrap()
            .contains("The best tools give you space.")
    );
    app.script_dialog("save-dialog.txt", &out("Report.md"));
    run_from_palette(&mut app, "markdown");
    dismiss_toast(&mut app);
    assert!(
        fs::read_to_string(out("Report.md"))
            .unwrap()
            .contains("# A little more room to think")
    );
    app.script_dialog("save-dialog.txt", &out("Report.docx"));
    run_from_palette(&mut app, "save as word");
    dismiss_toast(&mut app);
    assert_eq!(&fs::read(out("Report.docx")).unwrap()[..2], b"PK");
    assert_eq!(
        fs::read(&source).unwrap(),
        create_sample().unwrap(),
        "tools never touch the original"
    );

    println!("12 pages as pictures");
    script_folder(&app, &parts);
    run_from_palette(&mut app, "as pictures");
    app.wait(
        "document.querySelector('[role=dialog]')?.innerText.includes('Save pages as pictures')",
    );
    app.shot("pictures-sheet");
    app.click("Choose a folder");
    app.wait_text("Saved 4 pictures.");
    dismiss_toast(&mut app);
    let picture = parts.join("Report - page 1.png");
    assert!(picture.exists());

    println!("7 split, with a live count of what it will make");
    run_from_palette(&mut app, "split");
    app.fill("[role=dialog] input", "2");
    app.wait_text("Makes 2 PDFs");
    app.shot("split-sheet");
    app.click("Choose a folder");
    app.wait_text("Split into 2 PDFs.");
    dismiss_toast(&mut app);
    assert_eq!(
        Document::load_mem(&fs::read(parts.join("Report - part 2.pdf")).unwrap())
            .unwrap()
            .get_pages()
            .len(),
        2
    );

    println!("8 selecting thumbnails and saving them as a new PDF");
    let thumbnail = |index: usize| format!("document.querySelectorAll('.thumbnail')[{index}]");
    app.eval(&format!(
        "({}.dispatchEvent(new MouseEvent('click', {{ bubbles: true, ctrlKey: true }})), true)",
        thumbnail(0)
    ));
    app.eval(&format!(
        "({}.dispatchEvent(new MouseEvent('click', {{ bubbles: true, shiftKey: true }})), true)",
        thumbnail(2)
    ));
    app.wait_text("3 selected");
    app.shot("selection");
    app.script_dialog("save-dialog.txt", &out("Three.pdf"));
    app.click("Save as a new PDF");
    app.wait_text("Saved 3 pages as a new PDF.");
    dismiss_toast(&mut app);
    assert_eq!(
        Document::load_mem(&fs::read(out("Three.pdf")).unwrap())
            .unwrap()
            .get_pages()
            .len(),
        3
    );

    println!("9 dragging a thumbnail reorders pages, and undo puts it back");
    let first = app.center(&thumbnail(0));
    let third = app.center(&thumbnail(2));
    app.mouse("mouseMoved", first.0, first.1, 0);
    app.mouse("mousePressed", first.0, first.1, 1);
    for step in 1..=12 {
        let y = first.1 + (third.1 + 10.0 - first.1) * f64::from(step) / 12.0;
        app.mouse("mouseMoved", first.0, y, 1);
    }
    app.wait("document.querySelector('.thumbnail.lifted')");
    sleep(Duration::from_millis(240));
    app.shot("reorder-drag");
    app.mouse("mouseReleased", first.0, third.1 + 10.0, 0);
    app.wait("!document.querySelector('.thumbnail.lifted')");
    app.script_dialog("save-dialog.txt", &out("Reordered.pdf"));
    app.control('s');
    app.wait_text("Copy saved");
    dismiss_toast(&mut app);
    let reordered = Document::load_mem(&fs::read(out("Reordered.pdf")).unwrap()).unwrap();
    assert!(
        reordered
            .extract_text(&[3])
            .unwrap()
            .contains("A little more room"),
        "page one moved to third"
    );
    assert!(
        reordered
            .extract_text(&[1])
            .unwrap()
            .contains("Follow the thought")
    );
    app.click("Undo (Ctrl Z)");

    println!("1 highlights and notes export as Markdown");
    add_note(&mut app, "A thought worth keeping");
    app.wait_count(".annotation-layer text", 1);
    run_from_palette(&mut app, "notes");
    app.wait(
        "document.querySelector('.notes-preview')?.textContent.includes('A thought worth keeping')",
    );
    sleep(Duration::from_millis(280));
    app.shot("notes-sheet");
    app.script_dialog("save-dialog.txt", &out("Notes.md"));
    app.click("Save as a file");
    app.wait_text("Notes exported.");
    dismiss_toast(&mut app);
    assert!(
        fs::read_to_string(out("Notes.md"))
            .unwrap()
            .contains("## Page 1\n\nA thought worth keeping")
    );

    println!("15 signing: draw once, place, drag");
    app.press("s", 83, 0);
    app.wait("document.querySelector('.signature-pad')");
    let pad = app.center("document.querySelector('.signature-pad')");
    drag(
        &mut app,
        (pad.0 - 120.0, pad.1),
        (pad.0 + 120.0, pad.1 - 30.0),
    );
    app.wait("document.querySelector('.signature-pad path')");
    sleep(Duration::from_millis(280));
    app.shot("signature-sheet");
    app.click("Use this signature");
    app.wait("document.querySelector('.pdf-page.tool-sign .drawing-layer')");
    let page = app.eval("sized(document.querySelector('.pdf-page'))");
    let (left, top) = (
        page["left"].as_f64().unwrap(),
        page["top"].as_f64().unwrap(),
    );
    app.click_at(left + 300.0, top + 500.0);
    app.wait_count(".signature-mark", 1);
    let before = app
        .eval("document.querySelector('.signature-mark').getBoundingClientRect().left")
        .as_f64()
        .unwrap();
    let grab = app.center("document.querySelector('.signature-mark')");
    drag(&mut app, grab, (grab.0 + 60.0, grab.1 + 30.0));
    app.wait(&format!(
        "document.querySelector('.signature-mark').getBoundingClientRect().left > {before} + 40"
    ));

    println!("18 redaction removes text for good");
    app.press("r", 82, 0);
    app.wait("document.querySelector('.pdf-page.tool-redact .drawing-layer')");
    let span = app.eval("(() => { const span = [...document.querySelectorAll('.pdf-page[data-page=\"1\"] .textLayer span')].find((node) => node.textContent.includes('The best tools')); const box = span.getBoundingClientRect(); return { left: box.left, right: box.right, top: box.top, bottom: box.bottom }; })()");
    let number = |key: &str| span[key].as_f64().unwrap();
    drag(
        &mut app,
        (number("left") - 4.0, number("top") - 4.0),
        (number("right") + 4.0, number("bottom") + 4.0),
    );
    app.wait_count(".redaction-mark", 1);
    app.shot("redaction");
    app.script_dialog("save-dialog.txt", &out("Redacted.pdf"));
    app.control('s');
    app.wait_text("1 area removed for good");
    dismiss_toast(&mut app);
    let redacted = fs::read(out("Redacted.pdf")).unwrap();
    let text = pdf_text(&redacted);
    assert!(!text.contains("The best tools"), "redacted text survived");
    assert!(
        text.contains("Select a passage"),
        "other pages keep their text"
    );
    let saved = Document::load_mem(&redacted).unwrap();
    assert!(
        saved
            .objects
            .values()
            .filter_map(|object| object.as_stream().ok())
            .all(|stream| {
                let content = stream
                    .decompressed_content()
                    .unwrap_or_else(|_| stream.content.clone());
                !content.windows(14).any(|part| part == b"The best tools")
            })
    );
    let (_, marks) = split_marks(&redacted);
    assert!(
        marks
            .iter()
            .all(|mark| !matches!(mark.kind, MarkKind::Signature { .. })),
        "marks on a redacted page are burned in with it"
    );
    // Back to a clean document for the remaining tools.
    for _ in 0..3 {
        app.click("Undo (Ctrl Z)");
    }

    println!("15 a signature on an untouched page stays an editable annotation");
    app.press("s", 83, 0);
    app.wait("document.querySelector('.pdf-page.tool-sign .drawing-layer')");
    app.click_at(left + 300.0, top + 500.0);
    app.wait_count(".signature-mark", 1);
    app.script_dialog("save-dialog.txt", &out("Signed.pdf"));
    app.control('s');
    app.wait_text("Copy saved");
    dismiss_toast(&mut app);
    let (_, marks) = split_marks(&fs::read(out("Signed.pdf")).unwrap());
    assert!(
        marks
            .iter()
            .any(|mark| matches!(mark.kind, MarkKind::Signature { .. }))
    );
    app.click("Undo (Ctrl Z)");

    println!("19 a smaller copy reports what it saved");
    app.script_dialog("save-dialog.txt", &out("Smaller.pdf"));
    run_from_palette(&mut app, "smaller");
    app.wait("document.querySelector('.toast')?.innerText.includes('KB') || document.querySelector('.toast')?.innerText.includes('already as small')");
    dismiss_toast(&mut app);
    assert!(Document::load_mem(&fs::read(out("Smaller.pdf")).unwrap()).is_ok());

    println!("6 combining keeps the open document first");
    fs::write(
        app.profile.join("open-dialog.txt"),
        out("Three.pdf").to_string_lossy().as_bytes(),
    )
    .unwrap();
    run_from_palette(&mut app, "combine");
    app.wait("document.querySelector('.file-row:not(.current)')");
    sleep(Duration::from_millis(280));
    app.shot("combine-sheet");
    app.script_dialog("save-dialog.txt", &out("Combined.pdf"));
    app.click("Save");
    app.wait_text("Combined 2 PDFs.");
    dismiss_toast(&mut app);
    assert_eq!(
        Document::load_mem(&fs::read(out("Combined.pdf")).unwrap())
            .unwrap()
            .get_pages()
            .len(),
        7
    );

    println!("5 comparing finds the differences and jumps to them");
    fs::write(
        app.profile.join("open-dialog.txt"),
        out("Three.pdf").to_string_lossy().as_bytes(),
    )
    .unwrap();
    run_from_palette(&mut app, "compare");
    app.wait("document.querySelector('.change-row')");
    app.wait_text("1 change from Three.pdf");
    app.shot("compare");
    app.click("Done");
    app.wait_count(".change-row", 0);

    println!("16 protecting, then 17 removing the password");
    run_from_palette(&mut app, "password");
    app.fill("[role=dialog] input[type=password]", "secret");
    app.eval("document.querySelectorAll('[role=dialog] input')[1].focus()");
    app.send("Input.insertText", json!({ "text": "typo" }));
    app.wait_text("These don't match yet.");
    app.shot("protect-sheet");
    app.eval("document.querySelectorAll('[role=dialog] input')[1].select()");
    app.send("Input.insertText", json!({ "text": "secret" }));
    app.script_dialog("save-dialog.txt", &out("Locked.pdf"));
    app.wait("!named('Save protected copy').disabled");
    app.click("Save protected copy");
    app.wait_text("Saved a protected copy.");
    app.click("Open");
    app.wait("document.querySelector('[role=dialog]')?.innerText.includes('A little privacy')");
    app.fill("[aria-label='PDF password']", "secret");
    app.press("Enter", 13, 0);
    app.wait("document.querySelector('.document-heading')?.innerText.trim() === 'Locked'");
    app.rendered();
    app.script_dialog("save-dialog.txt", &out("Unlocked.pdf"));
    run_from_palette(&mut app, "remove");
    app.wait_text("opens without a password");
    dismiss_toast(&mut app);
    assert_eq!(
        Document::load_mem(&fs::read(out("Unlocked.pdf")).unwrap())
            .unwrap()
            .get_pages()
            .len(),
        4
    );

    println!("11 Word to PDF, from the Library");
    fs::write(
        app.profile.join("open-dialog.txt"),
        out("Report.docx").to_string_lossy().as_bytes(),
    )
    .unwrap();
    app.script_dialog("save-dialog.txt", &out("From Word.pdf"));
    run_from_palette(&mut app, "word to pdf");
    app.wait_text("Made a PDF from your Word document.");
    dismiss_toast(&mut app);
    assert!(pdf_text(&fs::read(out("From Word.pdf")).unwrap()).len() > 100);
    fs::write(
        app.profile.join("open-dialog.txt"),
        out("From Word.pdf").to_string_lossy().as_bytes(),
    )
    .unwrap();
    app.control('o');
    app.wait("document.querySelector('.document-heading')?.innerText.trim() === 'From Word'");
    app.rendered();
    sleep(Duration::from_millis(300));
    app.shot("word-to-pdf");

    println!("13 pictures to PDF, then 4 OCR makes the scan searchable");
    fs::write(
        app.profile.join("open-dialog.txt"),
        picture.to_string_lossy().as_bytes(),
    )
    .unwrap();
    run_from_palette(&mut app, "pictures to pdf");
    app.wait("document.querySelector('.file-row')");
    app.script_dialog("save-dialog.txt", &out("Scan.pdf"));
    app.click("Save");
    app.wait_text("Made a PDF from 1 picture.");
    app.click("Open");
    app.wait("document.querySelector('.document-heading')?.innerText.trim() === 'Scan'");
    app.wait("document.querySelector('.scan-hint')");
    sleep(Duration::from_millis(400));
    app.shot("scan-hint");
    app.script_dialog("save-dialog.txt", &out("Scan searchable.pdf"));
    app.click("Make searchable");
    app.wait("document.querySelector('.tool-status')");
    app.wait_text("It can now be searched.");
    app.click("Open");
    app.wait("document.querySelector('.document-heading')?.innerText.trim() === 'Scan searchable'");
    app.control('f');
    app.fill("[aria-label='Search document']", "attention");
    app.wait("document.querySelector('.search-result')");

    println!("14 filling in a form");
    let form = folder.0.join("Form.pdf");
    fs::write(&form, form_pdf()).unwrap();
    fs::write(
        app.profile.join("open-dialog.txt"),
        form.to_string_lossy().as_bytes(),
    )
    .unwrap();
    app.control('o');
    app.wait("document.querySelector('.document-heading')?.innerText.trim() === 'Form'");
    app.wait("document.querySelector('.form-field input')");
    sleep(Duration::from_millis(300));
    run_from_palette(&mut app, "fill");
    app.wait("document.activeElement?.closest('.form-field')");
    app.send("Input.insertText", json!({ "text": "Grace Hopper" }));
    app.press("Tab", 9, 0);
    app.eval("(document.activeElement.blur(), true)");
    app.eval("(document.querySelector('.form-field input[type=checkbox]').click(), true)");
    sleep(Duration::from_millis(300));
    app.shot("form");
    app.script_dialog("save-dialog.txt", &out("Filled.pdf"));
    app.control('s');
    app.wait_text("Copy saved");
    let fields = read_fields(&fs::read(out("Filled.pdf")).unwrap());
    assert_eq!(fields[0].value, "Grace Hopper");
    assert_eq!(fields[1].value, "Yes");

    println!("the new surfaces hold up in dark mode");
    app.eval("(named('Use dark appearance').click(), true)");
    app.wait("document.documentElement.dataset.theme === 'dark'");
    app.click("Tools (Ctrl K)");
    app.wait("document.activeElement?.getAttribute('aria-label') === 'Find a tool'");
    sleep(Duration::from_millis(250));
    app.shot("tools-list-dark");
    app.press("Escape", 27, 0);
    app.wait_count(".palette", 0);
    run_from_palette(&mut app, "protect");
    app.wait("document.querySelector('[role=dialog] input')");
    sleep(Duration::from_millis(300));
    app.shot("protect-dark");
    app.finish();
}

/// The first launch after an Electron release keeps its dark appearance.
#[test]
fn earlier_appearance_carries_over() {
    let folder = Folder::new("theme");
    let storage = folder
        .0
        .join("profile")
        .join("Local Storage")
        .join("leveldb");
    fs::create_dir_all(&storage).unwrap();
    let mut database =
        rusty_leveldb::DB::open(&storage, rusty_leveldb::Options::default()).unwrap();
    database
        .put(b"_file://\x00\x01ethos-theme", b"\x01dark")
        .unwrap();
    database.flush().unwrap();
    database.close().unwrap();
    let mut app = App::launch(&folder.0, 9350, &[]);
    app.wait("document.documentElement?.dataset.theme === 'dark'");
    app.wait("localStorage.getItem('ethos-theme') === 'dark'");
    app.finish();
}
