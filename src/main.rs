use anyhow::{Context, Result};
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use sanitize_filename::sanitize;
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[derive(Parser, Debug)]
#[command(name = "yt-clipper")]
#[command(about = "Split YouTube videos into chapters with multiple format variants", long_about = None)]
struct Args {
    #[arg(value_name = "URL")]
    url: String,

    #[arg(short, long)]
    keep_full: bool,

    #[arg(short, long)]
    formats: bool,
}

#[derive(Debug, Deserialize)]
struct VideoInfo {
    title: String,
    chapters: Option<Vec<Chapter>>,
}

#[derive(Debug, Deserialize)]
struct Chapter {
    title: String,
    start_time: f64,
    end_time: f64,
}

fn main() -> Result<()> {
    let args = Args::parse();

    println!("yt-clipper\n");

    check_dependency("yt-dlp")?;
    check_dependency("ffmpeg")?;

    let cleaned_url = clean_url(&args.url);

    println!("Fetching video information...");
    let video_info = get_video_info(&cleaned_url)?;

    println!("Video: {}", video_info.title);

    let chapters = video_info
        .chapters
        .context("No chapters found in this video")?;

    if chapters.is_empty() {
        anyhow::bail!("No chapters found in this video");
    }

    println!("Found {} chapters\n", chapters.len());

    let sanitized_title = sanitize(&video_info.title);
    let output_dir = PathBuf::from(".").join(&sanitized_title);
    let clips_dir = output_dir.join("clips");

    fs::create_dir_all(&clips_dir).context("Failed to create clips directory")?;

    println!("Output directory: {}\n", output_dir.display());

    let video_path = download_video(&cleaned_url, &output_dir)?;

    println!("\nSplitting video into chapters...\n");

    split_video_into_chapters(&video_path, &chapters, &clips_dir)?;

    if args.formats {
        println!("\nGenerating format variants...\n");
        let formats_dir = output_dir.join("formats");
        fs::create_dir_all(&formats_dir).context("Failed to create formats directory")?;
        generate_format_variants(&video_path, &chapters, &formats_dir)?;
    }

    if !args.keep_full {
        fs::remove_file(&video_path).context("Failed to remove full video file")?;
        println!("\nRemoved full video file");
    }

    println!("\nDone! All clips saved to: {}", output_dir.display());
    println!("  - Original clips: {}", clips_dir.display());
    if args.formats {
        let formats_dir = output_dir.join("formats");
        println!("  - Format variants: {}", formats_dir.display());
    }

    Ok(())
}

fn clean_url(url: &str) -> String {
    url.replace("\\?", "?")
        .replace("\\=", "=")
        .replace("\\&", "&")
}

fn check_dependency(name: &str) -> Result<()> {
    let output = Command::new(name).arg("--version").output();

    match output {
        Ok(_) => Ok(()),
        Err(_) => anyhow::bail!(
            "{} is not installed or not in PATH. Please install it first.\n\
             For yt-dlp: https://github.com/yt-dlp/yt-dlp#installation\n\
             For ff
mpeg: https://ffmpeg.org/download.html",
            name
        ),
    }
}

fn get_video_info(url: &str) -> Result<VideoInfo> {
    let output = Command::new("yt-dlp")
        .args(["--dump-json", "--no-download", url])
        .output()
        .context("Failed to execute yt-dlp")?;

    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("yt-dlp failed: {}", error);
    }

    let json_str = String::from_utf8(output.stdout).context("Failed to parse yt-dlp output")?;

    let video_info: VideoInfo =
        serde_json::from_str(&json_str).context("Failed to parse video information")?;

    Ok(video_info)
}

fn download_video(url: &str, output_dir: &PathBuf) -> Result<PathBuf> {
    println!("Downloading video at highest quality...");

    let output_template = output_dir.join("full_video.%(ext)s");
    let output_template_str = output_template.to_str().context("Invalid output path")?;

    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {msg}")
            .unwrap(),
    );
    pb.set_message("Downloading...");

    let status = Command::new("yt-dlp")
        .args([
            "-f",
            "bestvideo+bestaudio/best",
            "--merge-output-format",
            "mp4",
            "-o",
            output_template_str,
            url,
        ])
        .status()
        .context("Failed to execute yt-dlp")?;

    pb.finish_and_clear();

    if !status.success() {
        anyhow::bail!("Failed to download video");
    }

    let video_path = output_dir.join("full_video.mp4");

    if !video_path.exists() {
        anyhow::bail!("Downloaded video file not found");
    }

    println!("Download complete");

    Ok(video_path)
}

fn split_video_into_chapters(
    video_path: &PathBuf,
    chapters: &[Chapter],
    output_dir: &PathBuf,
) -> Result<()> {
    let pb = ProgressBar::new(chapters.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] {bar:40.cyan/blue} {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("=>-"),
    );

    for (i, chapter) in chapters.iter().enumerate() {
        let chapter_num = format!("{:02}", i + 1);
        let sanitized_chapter_title = sanitize(&chapter.title);
        let output_filename = format!("{}_{}.mp4", chapter_num, sanitized_chapter_title);
        let output_path = output_dir.join(output_filename);

        pb.set_message(format!("Processing: {}", chapter.title));

        let duration = chapter.end_time - chapter.start_time;

        let status = Command::new("ffmpeg")
            .args([
                "-i",
                video_path.to_str().unwrap(),
                "-ss",
                &format!("{:.3}", chapter.start_time),
                "-t",
                &format!("{:.3}", duration),
                "-c",
                "copy",
                "-avoid_negative_ts",
                "1",
                "-y",
                output_path.to_str().unwrap(),
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .context("Failed to execute ffmpeg")?;

        if !status.success() {
            pb.finish_and_clear();
            anyhow::bail!("Failed to split chapter: {}", chapter.title);
        }

        pb.inc(1);
    }

    pb.finish_with_message("All chapters processed");

    Ok(())
}

fn generate_format_variants(
    video_path: &PathBuf,
    chapters: &[Chapter],
    formats_dir: &PathBuf,
) -> Result<()> {
    let vertical_dir = formats_dir.join("vertical");
    let audio_only_dir = formats_dir.join("audio_only");
    let no_audio_dir = formats_dir.join("no_audio");

    fs::create_dir_all(&vertical_dir)?;
    fs::create_dir_all(&audio_only_dir)?;
    fs::create_dir_all(&no_audio_dir)?;

    let total_tasks = chapters.len() * 3;
    let pb = ProgressBar::new(total_tasks as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] {bar:40.cyan/blue} {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("=>-"),
    );

    for (i, chapter) in chapters.iter().enumerate() {
        let chapter_num = format!("{:02}", i + 1);
        let sanitized_chapter_title = sanitize(&chapter.title);
        let base_filename = format!("{}_{}", chapter_num, sanitized_chapter_title);
        let duration = chapter.end_time - chapter.start_time;
        let start_time = format!("{:.3}", chapter.start_time);
        let duration_str = format!("{:.3}", duration);

        pb.set_message(format!("Vertical: {}", chapter.title));
        let vertical_output = vertical_dir.join(format!("{}.mp4", base_filename));
        Command::new("ffmpeg")
            .args([
                "-i",
                video_path.to_str().unwrap(),
                "-ss",
                &start_time,
                "-t",
                &duration_str,
                "-vf",
                "crop=ih*9/16:ih",
                "-c:a",
                "copy",
                "-avoid_negative_ts",
                "1",
                "-y",
                vertical_output.to_str().unwrap(),
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .context("Failed to create vertical format")?;
        pb.inc(1);

        pb.set_message(format!("Audio only: {}", chapter.title));
        let audio_output = audio_only_dir.join(format!("{}.mp3", base_filename));
        Command::new("ffmpeg")
            .args([
                "-i",
                video_path.to_str().unwrap(),
                "-ss",
                &start_time,
                "-t",
                &duration_str,
                "-vn",
                "-acodec",
                "libmp3lame",
                "-q:a",
                "2",
                "-y",
                audio_output.to_str().unwrap(),
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .context("Failed to create audio only format")?;
        pb.inc(1);

        pb.set_message(format!("No audio: {}", chapter.title));
        let no_audio_output = no_audio_dir.join(format!("{}.mp4", base_filename));
        Command::new("ffmpeg")
            .args([
                "-i",
                video_path.to_str().unwrap(),
                "-ss",
                &start_time,
                "-t",
                &duration_str,
                "-an",
                "-c:v",
                "copy",
                "-avoid_negative_ts",
                "1",
                "-y",
                no_audio_output.to_str().unwrap(),
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .context("Failed to create no audio format")?;
        pb.inc(1);
    }

    pb.finish_with_message("All format variants generated");

    Ok(())
}
