mod audio;
mod ptt;

use std::io::{Write, stdout};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use clap::{Parser, Subcommand};
use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{self, disable_raw_mode, enable_raw_mode};

use audio::{Capture, Recording};
use ptt::{HoldDetector, Transition};

#[derive(Parser)]
#[command(
    name = "voice",
    version,
    about = "Conversa por voz com agentes de código no terminal"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Grava o microfone e toca de volta, para testar o áudio.
    MicTest {
        /// Grava por N segundos sem push-to-talk.
        #[arg(long)]
        seconds: Option<f32>,
        /// Tecla de push-to-talk.
        #[arg(long, default_value_t = ' ')]
        key: char,
        /// Não toca a gravação de volta.
        #[arg(long)]
        no_play: bool,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::MicTest {
            seconds: Some(secs),
            no_play,
            ..
        } => timed_test(secs, !no_play),
        Command::MicTest { key, no_play, .. } => push_to_talk_test(key, !no_play),
    }
}

fn timed_test(secs: f32, play: bool) -> Result<()> {
    let capture = Capture::start()?;
    println!("Gravando {secs:.1} s...");
    capture.begin();
    thread::sleep(Duration::from_secs_f32(secs));
    let rec = capture.end();
    report(&rec, None, "\n");
    if play {
        playback(&rec.samples, "\n")?;
    }
    Ok(())
}

/// Keeps the terminal in raw mode for its lifetime and restores it on drop,
/// including on errors.
struct RawTerminal {
    enhanced: bool,
}

impl RawTerminal {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        // Ask for key-release events where the terminal supports them.
        let enhanced = terminal::supports_keyboard_enhancement().unwrap_or(false);
        if enhanced {
            execute!(
                stdout(),
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::REPORT_EVENT_TYPES)
            )?;
        }
        Ok(Self { enhanced })
    }
}

impl Drop for RawTerminal {
    fn drop(&mut self) {
        if self.enhanced {
            let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
        }
        let _ = disable_raw_mode();
    }
}

fn push_to_talk_test(key: char, play: bool) -> Result<()> {
    let capture = Capture::start()?;
    let term = RawTerminal::enter()?;
    let nl = "\r\n";
    let key_name = if key == ' ' {
        "ESPAÇO".to_string()
    } else {
        format!("'{key}'")
    };

    print!("Segure {key_name} para gravar e solte para ouvir. q ou Esc para sair.{nl}");
    if term.enhanced {
        print!("(o terminal informa quando a tecla é solta){nl}");
    } else {
        print!("(o terminal não informa a soltura; usando a repetição automática da tecla){nl}");
    }
    stdout().flush()?;

    let mut detector = HoldDetector::default();
    let mut started_at = Instant::now();
    let mut last_draw = Instant::now();

    loop {
        let mut transition = None;

        if event::poll(Duration::from_millis(10))?
            && let Event::Key(k) = event::read()?
        {
            let ctrl_c =
                k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL);
            if ctrl_c || k.code == KeyCode::Esc || k.code == KeyCode::Char('q') && key != 'q' {
                break;
            }
            if k.code == KeyCode::Char(key) {
                transition = match k.kind {
                    KeyEventKind::Press | KeyEventKind::Repeat => detector.press(Instant::now()),
                    KeyEventKind::Release => detector.release(),
                };
            }
        }
        if transition.is_none() {
            transition = detector.tick(Instant::now());
        }

        match transition {
            Some(Transition::Started) => {
                capture.begin();
                started_at = Instant::now();
            }
            Some(Transition::Stopped { lag }) => {
                let rec = capture.end();
                print!("\r{}", " ".repeat(60));
                report(&rec, Some(lag), nl);
                if play {
                    playback(&rec.samples, nl)?;
                }
                print!("{nl}Pronto. Segure {key_name} de novo ou q para sair.{nl}");
                stdout().flush()?;
            }
            None => {}
        }

        if detector.is_holding() && last_draw.elapsed() >= Duration::from_millis(50) {
            draw_meter(started_at.elapsed(), capture.level())?;
            last_draw = Instant::now();
        }
    }

    drop(term);
    println!();
    Ok(())
}

fn draw_meter(elapsed: Duration, level: f32) -> Result<()> {
    const WIDTH: usize = 30;
    // Map -60..0 dBFS onto the bar.
    let db = if level > 0.0 {
        20.0 * level.log10()
    } else {
        -60.0
    };
    let filled = (((db + 60.0) / 60.0).clamp(0.0, 1.0) * WIDTH as f32) as usize;
    print!(
        "\r● gravando {:>4.1}s [{}{}]",
        elapsed.as_secs_f32(),
        "#".repeat(filled),
        " ".repeat(WIDTH - filled)
    );
    stdout().flush()?;
    Ok(())
}

fn report(rec: &Recording, lag: Option<Duration>, nl: &str) {
    let s = audio::stats(&rec.samples);
    print!(
        "\rGravado: {:.2} s | pico {:.1} dBFS | média {:.1} dBFS{nl}",
        s.seconds, s.peak_dbfs, s.rms_dbfs
    );
    print!(
        "Espera pelo fim do áudio: {} ms.{nl}",
        rec.tail_wait.as_millis()
    );
    match lag {
        Some(lag) if lag > Duration::ZERO => print!(
            "Soltura detectada {} ms depois do último evento da tecla.{nl}",
            lag.as_millis()
        ),
        Some(_) => print!("Soltura informada pelo terminal na hora.{nl}"),
        None => {}
    }
    if s.peak_dbfs < -45.0 {
        print!(
            "Aviso: parece silêncio. Confira no Windows em Configurações > Privacidade > Microfone \
             se os apps de desktop podem usar o microfone.{nl}"
        );
    }
    let _ = stdout().flush();
}

fn playback(samples: &[i16], nl: &str) -> Result<()> {
    print!("Tocando de volta...{nl}");
    stdout().flush()?;
    let t = Instant::now();
    audio::play(samples)?;
    print!(
        "Reprodução terminou em {:.2} s.{nl}",
        t.elapsed().as_secs_f32()
    );
    stdout().flush()?;
    Ok(())
}
