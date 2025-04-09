use std::{env::args, error::Error, io::Read, path::PathBuf};

use ffmpeg_sidecar::{command::{ffmpeg_is_installed, FfmpegCommand}, ffprobe::ffprobe_is_installed};

fn main() -> Result<(), Box<dyn Error>> {
    if !ffmpeg_is_installed() || !ffprobe_is_installed() {
        panic!("not installed")
    }

    let in_dir = args().nth(1).expect("Please provide an input directory :)").parse::<PathBuf>()?.canonicalize().unwrap();
    let out_file = args().nth(2).expect("PLEASE provide an out file >:(").parse::<PathBuf>()?;

    if !in_dir.is_dir() {
        return Err(format!("not a dir: {}", in_dir.display()).into());
    }

    let mut command = FfmpegCommand::new();
    let mut video_count: usize = 0;

    let mut files = in_dir.read_dir().unwrap();
    while let Some(Ok(file)) = files.next() {
        let path = file.path();
        if path.extension().is_some_and(|ext| ext.eq("mp4")) {
            command.input(path.to_str().unwrap());
            video_count+=1;
        }
    }

    if video_count < 2 {
        return Err("you need at least 2 videos buh".into());
    }

    let mut filter_string = String::new();
    let mut concat_string = String::new();


    for i in 0..video_count {
        let var = num2words::Num2Words::new(i as i32).to_words().unwrap();
        filter_string.push_str(format!("[{i}:v]scale=1920:1080:force_original_aspect_ratio=decrease,pad=ih*16/9/sar:ih:(ow-iw)/2:(oh-ih)/2,setsar=sar=1/1[{var}];").as_str());
        concat_string.push_str(format!("[{var}][{i}:a]").as_str());
    }

    filter_string = format!("{filter_string} {concat_string} concat=n={video_count}:v=1:a=1[v][a]");

    let mut buf = String::new();
    let mut command = command
    .filter_complex(filter_string)
    .args(format!("-map [v] -map [a]").split(' '))
    .codec_video("libx265")
    .output(out_file.with_extension("mp4").to_str().unwrap())
    .overwrite()
    .print_command()
    .spawn()
    .unwrap();

    command.take_stderr()
    .unwrap()
    .read_to_string(&mut buf)
    .unwrap();

    command
    .wait()
    .unwrap();

    println!("{buf}");

    Ok(())
}
