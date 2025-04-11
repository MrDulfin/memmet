use std::{env::args, error::Error, fmt::Debug, io::Read, path::PathBuf};

use ffmpeg_sidecar::{command::{ffmpeg_is_installed, FfmpegCommand}, ffprobe::ffprobe_is_installed};
use ffprobe::{ffprobe, FfProbeError};

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

    let inputs = in_dir
        .read_dir()
        .unwrap()
        .enumerate()
        .filter_map(|(i, file)| {
            if let Ok(file) = file {
                let path = file.path();
                if path.extension().is_some_and(|ext| ext.eq("mp4") || ext.eq("mkv")) {
                    let input = check_for_tracks(&path).unwrap();
                    if input.colour_space.as_str() != "reserved" {
                        command.input(path.to_str().unwrap());
                        Some(input)
                    } else {
                        None
                    }

                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    command.args("-f lavfi -i anullsrc=channel_layout=stereo".split(' '));
    let len = inputs.len();
    if len < 2 {
        return Err("you need at least 2 videos buh".into());
    }

    let mut filter_string = String::new();
    let mut concat_string = String::new();

    for (i, input) in inputs.iter().enumerate() {
        let var = num2words::Num2Words::new(i as i32).to_words().unwrap();
        let InputFile { audio, .. } = input;
        filter_string.push_str(format!("[{i}:v]scale=1920:1080:force_original_aspect_ratio=decrease,setdar=ratio=16/9,setsar=sar=1/1,pad=ih*16/9/sar:ih:(ow-iw)/2:(oh-ih)/2[{var}];").as_str());
        concat_string.push_str(format!("[{var}]").as_str());
        concat_string.push_str( match audio {
            Some(_) => format!("[{i}:a]"),
            // to use the last audio track(our filler audio)
            None =>  format!("[{len}]")
        }.as_str());
    }


    filter_string = format!("{filter_string} {concat_string} concat=n={}:v=1:a=1[v][a]", len);

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

    command.wait().unwrap();
    println!("{buf}");
    Ok(())
}

#[derive(Default, Debug)]
struct InputFile {
    video: usize,
    audio: Option<usize>,
    colour_space: String,
}

fn check_for_tracks(path: impl AsRef<std::path::Path> + Debug) -> Result<InputFile, FfProbeError> {
    // dbg!(&path);
    let info = ffprobe(&path)?;
    let mut input = InputFile::default();

    let mut video_selected = false;
    let mut audio_selected = false;

    for stream in info.streams {
        let codec = stream.codec_type.as_ref().map(|s| s.as_str());
        match codec {
            Some("video") => {
                if !video_selected {
                    video_selected = true;
                    input.video = stream.index as usize;
                }
            }
            Some("audio") => {
                if !audio_selected {
                    audio_selected = true;
                    input.audio = Some(stream.index as usize)
                }
            }
            _ => continue
        }
        if let Some(colour_space) = stream.color_space {
            input.colour_space = colour_space;
        }
    }

    Ok(input)
}