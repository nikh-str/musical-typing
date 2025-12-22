
use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use rand::{distributions::Distribution, seq::SliceRandom, thread_rng, Rng};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Terminal,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    io::{self},
    process::{Command as SysCommand, Stdio},
    time::{Duration, Instant},
};

const DEFAULT_WORDS_STR: &str = "the be to of and a in that have I it for not on with he as you do at this but his by from they we say her she or an will my one all would there their what so up out if about who get which go me when make can like time no just him know take people into year your good some could them see other than then now look only come its over think also back after use two how our work first well way even new want because any these give day most us";

// --- Gum Integration Wrappers ---

fn gum_choose(header: &str, options: &[&str]) -> Result<String> {
    let child = SysCommand::new("gum")
        .arg("choose")
        .arg("--item.foreground").arg("240")     // Dark Grey text when unselected
        .arg("--selected.foreground").arg("255") // White text when selected
        .arg("--cursor.foreground").arg("#07CE41")   // cursor
        .arg("--header")
        .arg(header)
        .args(options)
        .stdin(Stdio::inherit())  // Allow keyboard input
        .stderr(Stdio::inherit()) // Allow gum to draw the menu
        .stdout(Stdio::piped())   // Capture the selection
        .spawn()
        .context("Failed to spawn gum. Is it installed?")?;

    // 2. Center the Header using `gum style`
    // We render the header separately to give it a nice border/background
    let _ = SysCommand::new("gum")
        .arg("style")
        .arg("--foreground").arg("#36AA92")
        .arg("--background").arg("0000000")       // Real Black background
        .arg("--border").arg("rounded")
        .arg("--border-foreground").arg("#E7AFF6")
        .arg("--padding").arg("0 2")
        .arg("--margin").arg("0 0 1 0")     // Margin bottom
        .arg("--align").arg("center")
        .arg("--width").arg("50")           // Fixed width container
        .arg(header)
        .status();

    let output = child.wait_with_output()?;

    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn gum_input(header: &str, placeholder: &str, value: &str) -> Result<String> {
    let output = SysCommand::new("gum")
        .arg("input")
        .arg("--header")
        .arg(header)
        .arg("--placeholder")
        .arg(placeholder)
        .arg("--value")
        .arg(value)
        .output()
        .context("Failed to execute gum")?;

    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn gum_confirm(prompt: &str) -> bool {
    SysCommand::new("gum")
        .arg("confirm")
        .arg(prompt)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn gum_style(text: &str) -> Result<()> {
    SysCommand::new("gum")
        .arg("style")
        .arg("--border")
        .arg("double")
        .arg("--margin")
        .arg("1 1")
        .arg("--padding")
        .arg("1 2")
        .arg("--border-foreground")
        .arg("212") // Pinkish color
        .arg(text)
        .status()?;
    Ok(())
}

// --- Data Structures ---

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Settings {
    forgive_errors: bool,
    default_time_limit: u64,
    default_words_limit: usize,
    show_wpm_live: bool,
    auto_save_results: bool,
    min_accuracy_to_save: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            forgive_errors: false,
            default_time_limit: 60,
            default_words_limit: 25,
            show_wpm_live: true,
            auto_save_results: true,
            min_accuracy_to_save: 0.5,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct TestResult {
    timestamp: DateTime<Local>,
    raw_wpm: f64,
    wpm: f64,
    accuracy: f64,
    time_taken: f64,
    text_length: usize,
    words_typed: usize,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
struct UserData {
    letter_shown: HashMap<char, u32>,
    letter_correct: HashMap<char, u32>,
    letter_accuracy: HashMap<char, f64>,
    letter_time_total: HashMap<char, f64>,
    letter_time_count: HashMap<char, u32>,
    letter_wpm: HashMap<char, f64>,
    test_history: Vec<TestResult>,
}

struct AppState {
    settings: Settings,
    user_data: UserData,
    words_list: Vec<String>,
}

impl AppState {
    fn load() -> Self {
        let settings = fs::read_to_string("settings.json")
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

        let user_data = fs::read_to_string("userdata.json")
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

        let words_list = fs::read_to_string("words.txt")
            .ok()
            .map(|s| s.lines().map(|l| l.trim().to_string()).collect())
            .unwrap_or_else(|| {
                DEFAULT_WORDS_STR
                    .split_whitespace()
                    .map(|s| s.to_string())
                    .collect()
            });

        Self {
            settings,
            user_data,
            words_list,
        }
    }

    fn save(&self) {
        if let Ok(json) = serde_json::to_string_pretty(&self.settings) {
            let _ = fs::write("settings.json", json);
        }
        if let Ok(json) = serde_json::to_string_pretty(&self.user_data) {
            let _ = fs::write("userdata.json", json);
        }
    }

    // Algorithm to select words based on user weakness (High Frequency + Low Accuracy)
    fn get_weighted_words(&self, count: usize) -> String {
        let mut rng = thread_rng();
        
        // Standard English frequency 
        let frequency: HashMap<char, f64> = HashMap::from([
            ('e', 12.02), ('t', 9.10), ('a', 8.12), ('o', 7.68), ('i', 7.31), ('n', 6.95),
            ('s', 6.28), ('r', 6.02), ('h', 5.92), ('d', 4.32), ('l', 3.98), ('u', 2.88),
            ('c', 2.71), ('m', 2.61), ('f', 2.30), ('y', 2.11), ('w', 2.09), ('g', 2.03),
            ('p', 1.82), ('b', 1.49), ('v', 1.11), ('k', 0.69), ('x', 0.17), ('q', 0.11),
            ('j', 0.10), ('z', 0.07),
        ]);

        let mut letter_weight = HashMap::new();
        for ch in ' '..='~' {
            let acc = *self.user_data.letter_accuracy.get(&ch).unwrap_or(&0.0);
            let wpm = *self.user_data.letter_wpm.get(&ch).unwrap_or(&0.0);
            
            // If accuracy is high, weight is low. If accuracy is low, weight is high.
            let inv_acc = if acc > 0.01 { 1.0 / acc } else { 20.0 };
            let wpm_weight = 1.0 / (wpm + 0.1);

            if let Some(freq) = frequency.get(&ch) {
                letter_weight.insert(ch, inv_acc * freq * wpm_weight);
            } else {
                letter_weight.insert(ch, 1.0);
            }
        }

        let mut word_weights = Vec::with_capacity(self.words_list.len());
        for word in &self.words_list {
            let mut weight = 0.0;
            let mut len = 0.0;
            for ch in word.chars() {
                let w = letter_weight.get(&ch).unwrap_or(&1.0);
                weight += w;
                len += 1.0;
            }
            if len > 0.0 {
                word_weights.push(weight / len);
            } else {
                word_weights.push(0.0);
            }
        }

        let mut chosen_words = Vec::new();
        if let Ok(dist) = rand::distributions::WeightedIndex::new(&word_weights) {
            for _ in 0..count {
                chosen_words.push(self.words_list[dist.sample(&mut rng)].clone());
            }
        } else {
            // Fallback
            for _ in 0..count {
                chosen_words.push(self.words_list.choose(&mut rng).unwrap().clone());
            }
        }

        chosen_words.join(" ")
    }

    fn update_stats(&mut self, char: char, is_correct: bool, time_taken: f64) {
        let shown = self.user_data.letter_shown.entry(char).or_insert(0);
        *shown += 1;
        
        if is_correct {
            *self.user_data.letter_correct.entry(char).or_insert(0) += 1;
            *self.user_data.letter_time_total.entry(char).or_insert(0.0) += time_taken;
            *self.user_data.letter_time_count.entry(char).or_insert(0) += 1;
        }

        let s = *self.user_data.letter_shown.get(&char).unwrap_or(&0) as f64;
        let c = *self.user_data.letter_correct.get(&char).unwrap_or(&0) as f64;
        
        if s > 0.0 {
            self.user_data.letter_accuracy.insert(char, c / s);
        }

        let total_time = *self.user_data.letter_time_total.get(&char).unwrap_or(&0.0);
        let count = *self.user_data.letter_time_count.get(&char).unwrap_or(&0);
        if count > 0 && total_time > 0.0 {
             let avg = total_time / count as f64;
             self.user_data.letter_wpm.insert(char, 12.0 / avg);
        }
    }
}

// --- TUI Game Loop ---

#[derive(PartialEq)]
enum TestMode {
    Time(u64),
    Words(usize),
    Forever,
}

fn run_test(app: &mut AppState, mode: TestMode) -> Result<Option<TestResult>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let target_count = match mode {
        TestMode::Words(n) => n,
        TestMode::Time(_) | TestMode::Forever => 50,
    };
    let mut target_text = app.get_weighted_words(target_count);
    let mut input_text = String::new();
    
    let mut last_keystroke = Instant::now();
    let mut is_started = false;
    let mut real_start_time = Instant::now();
    
    let mut should_exit = false;
    let mut completed = false;
    let mut scroll_offset = 0;

    while !should_exit && !completed {
        let elapsed = if is_started { real_start_time.elapsed() } else { Duration::from_secs(0) };
        let wpm = if elapsed.as_secs_f64() > 0.0 {
             (input_text.len() as f64 / 5.0) / (elapsed.as_secs_f64() / 60.0)
        } else {
            0.0
        };

        // Check if Time Mode is finished
        if let TestMode::Time(limit) = mode {
            if is_started && elapsed.as_secs() >= limit {
                completed = true;
                break;
            }
        }

        // Buffer management for continuous modes
        if matches!(mode, TestMode::Time(_) | TestMode::Forever) {
            if input_text.len() + 50 > target_text.len() {
                let more = app.get_weighted_words(20);
                target_text.push(' ');
                target_text.push_str(&more);
            }
        }

        // Draw UI
        terminal.draw(|f| {
            let layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Fill(1),
                    Constraint::Length(12),
                    Constraint::Min(1),
                    Constraint::Length(1),
                ])
                .split(f.size());

            // Header Area
            let mode_str = match mode {
                TestMode::Time(t) => format!("Time Mode: {}s", t),
                TestMode::Words(w) => format!("Words Mode: {}", w),
                TestMode::Forever => "Forever Mode".to_string(),
            };
            
            let status = if is_started {
                match mode {
                    TestMode::Time(limit) => format!("{} | Time Left: {:.0}s | WPM: {:.0}", mode_str, (limit as f64 - elapsed.as_secs_f64()).max(0.0), wpm),
                    _ => format!("{} | Time: {:.0}s | WPM: {:.0}", mode_str, elapsed.as_secs_f64(), wpm),
                }
            } else {
                format!("{} | Press any key to start typing...", mode_str)
            };

            f.render_widget(
                Paragraph::new(status).bg(Color::Rgb(58, 7, 20)).bold().alignment(Alignment::Center).block(Block::default().borders(Borders::BOTTOM)),
                layout[0]
            );

            // Typing Text Area
            let width = layout[1].width as usize;
            let visible_lines = layout[1].height as usize;
            let cursor_row = input_text.len() / width;
            
            // Auto scroll
            if cursor_row > scroll_offset + visible_lines / 2 {
                scroll_offset = cursor_row - visible_lines / 2;
            }
            
            let mut spans = Vec::new();
            let start_char_idx = scroll_offset * width;
            
            if start_char_idx < target_text.len() {
                let mut current_line = vec![];
                let visible_text: Vec<(usize, char)> = target_text
                    .char_indices()
                    .skip(start_char_idx)
                    .take(visible_lines * width)
                    .collect();

                let mut current_width = 0;

                for (absolute_idx, c) in visible_text {
                    let style = if absolute_idx < input_text.len() {
                        let inputted = input_text.chars().nth(absolute_idx).unwrap();
                        if inputted == c {
                            Style::default().fg(Color::Green)
                        } else {
                            Style::default().fg(Color::Red).add_modifier(Modifier::UNDERLINED)
                        }
                    } else if absolute_idx == input_text.len() {
                        Style::default().fg(Color::Blue).add_modifier(Modifier::UNDERLINED | Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Gray)
                    };

                    current_line.push(Span::styled(c.to_string(), style));
                    current_width += 1;

                    if current_width >= width {
                        spans.push(Line::from(current_line));
                        current_line = vec![];
                        current_width = 0;
                    }
                }
                if !current_line.is_empty() {
                    spans.push(Line::from(current_line));
                }
            }

            f.render_widget(
                Paragraph::new(spans).block(Block::default().padding(ratatui::widgets::Padding::new(2,2,1,1)))
                .style(Style::default().bg(Color::Rgb(20, 20, 20))), 
                layout[1]
            );

            // Footer Area
            f.render_widget(
                Paragraph::new("ESC: Quit").alignment(Alignment::Center).style(Style::default().fg(Color::Gray).bg(Color::Black)),
                layout[2]
            );

        })?; // End of draw closure

        // Input Handling
        if event::poll(Duration::from_millis(16))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Esc => should_exit = true,
                        KeyCode::Backspace => {
                            if !input_text.is_empty() {
                                input_text.pop();
                            }
                        }
                        KeyCode::Char(c) => {
                            if !is_started {
                                is_started = true;
                                real_start_time = Instant::now();
                                last_keystroke = real_start_time;
                            }

                            // Process character if text not done
                            if input_text.len() < target_text.len() {
                                let now = Instant::now();
                                let delta = now.duration_since(last_keystroke).as_secs_f64();
                                last_keystroke = now;

                                let target_char = target_text.chars().nth(input_text.len()).unwrap();
                                let is_correct = c == target_char;
                                
                                app.update_stats(target_char, is_correct, delta);

                                if is_correct || !app.settings.forgive_errors {
                                    input_text.push(c);
                                } else if app.settings.forgive_errors && !is_correct {
                                    // Block input (do nothing)
                                }
                            }

                            // Check Word Limit Completion
                            if let TestMode::Words(limit) = mode {
                                let words_typed = input_text.split_whitespace().count();
                                if words_typed >= limit && input_text.ends_with(' ') {
                                    completed = true;
                                }
                                if input_text.len() == target_text.len() {
                                    completed = true;
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    } // End of While Loop

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    if completed {
        let elapsed = real_start_time.elapsed().as_secs_f64();
        let chars = input_text.len();
        let words = input_text.split_whitespace().count();
        let raw_wpm = (chars as f64 / 5.0) / (elapsed / 60.0);
        
        let mut correct_chars = 0;
        for (i, c) in input_text.chars().enumerate() {
            if i < target_text.len() && target_text.chars().nth(i) == Some(c) {
                correct_chars += 1;
            }
        }
        let accuracy = if chars > 0 { correct_chars as f64 / chars as f64 } else { 0.0 };
        let net_wpm = raw_wpm * accuracy;

        Ok(Some(TestResult {
            timestamp: Local::now(),
            raw_wpm,
            wpm: net_wpm,
            accuracy: accuracy * 100.0,
            time_taken: elapsed,
            text_length: chars,
            words_typed: words,
        }))
    } else {
        Ok(None)
    }
}

// --- Menus ---

fn settings_menu(app: &mut AppState) -> Result<()> {
    loop {
        // Clone simple Copy types to avoid borrow issues
        let options = vec![
            format!("Forgive Errors: {}", if app.settings.forgive_errors { "On" } else { "Off" }),
            format!("Default Time: {}s", app.settings.default_time_limit),
            format!("Default Words: {}", app.settings.default_words_limit),
            format!("Live WPM: {}", if app.settings.show_wpm_live { "On" } else { "Off" }),
            "Reset History".to_string(),
            "Back".to_string()
        ];
        
        let opts_str: Vec<&str> = options.iter().map(|s| s.as_str()).collect();
        let selection = gum_choose("Settings", &opts_str)?;

        if selection.starts_with("Back") {
            break;
        } else if selection.starts_with("Forgive") {
            app.settings.forgive_errors = !app.settings.forgive_errors;
        } else if selection.starts_with("Live WPM") {
            app.settings.show_wpm_live = !app.settings.show_wpm_live;
        } else if selection.starts_with("Default Time") {
            let val = gum_input("Set Time Limit (seconds)", "60", &app.settings.default_time_limit.to_string())?;
            if let Ok(n) = val.parse() { app.settings.default_time_limit = n; }
        } else if selection.starts_with("Default Words") {
            let val = gum_input("Set Word Limit", "25", &app.settings.default_words_limit.to_string())?;
            if let Ok(n) = val.parse() { app.settings.default_words_limit = n; }
        } else if selection.starts_with("Reset History") {
            if gum_confirm("Are you sure?") {
                app.user_data = UserData::default();
            }
        }
    }
    app.save();
    Ok(())
}


fn show_results(res: TestResult) -> Result<()> {
    let text = format!(
        "WPM: {:.2}\nRaw WPM: {:.2}\nAccuracy: {:.2}%\nTime: {:.2}s\nWords: {}",
        res.wpm, res.raw_wpm, res.accuracy, res.time_taken, res.words_typed
    );
    gum_style(&text)?;
    // Pause for user
    let _ = SysCommand::new("gum").arg("format").arg("Press Enter...").status();
    let _ = std::io::stdin().read_line(&mut String::new());
    Ok(())
}

fn main() -> Result<()> {
    // Check for gum installation
    if SysCommand::new("gum").arg("--version").output().is_err() {
        eprintln!("Error: 'gum' is not installed (https://github.com/charmbracelet/gum).");
        return Ok(());
    }

    let mut app = AppState::load();

    loop {
        let _ = SysCommand::new("clear").status();
        let selection = gum_choose(
            "TYPR - Rust Edition", 
            &["Start Words Test", "Start Time Test", "Forever Mode", "Settings", "Exit"]
        )?;

        let result = match selection.as_str() {
            "Start Words Test" => {
                let limit = app.settings.default_words_limit;
                run_test(&mut app, TestMode::Words(limit))?
            },
            "Start Time Test" => {
                let limit = app.settings.default_time_limit;
                run_test(&mut app, TestMode::Time(limit))?
            },
            "Forever Mode" => {
                run_test(&mut app, TestMode::Forever)?
            },
            "Settings" => {
                settings_menu(&mut app)?;
                None
            },
            "Exit" | "Back" | "" => break,
            _ => None,
        };

        if let Some(res) = result {
            if app.settings.auto_save_results && res.accuracy >= app.settings.min_accuracy_to_save * 100.0 {
                 app.user_data.test_history.push(res.clone());
                    app.save();
        }
        show_results(res)?;
        }
    } // End of Main Loop
    Ok(())
}

