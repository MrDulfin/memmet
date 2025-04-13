use std::{
    error::Error,
    fmt::Debug,
    fs::{self, OpenOptions},
    io::{BufRead, Read, Write, stdout},
    path::PathBuf,
};

use clap::{Arg, ArgAction, ArgMatches, Command, value_parser};
use ffmpeg_sidecar::{
    command::{FfmpegCommand, ffmpeg_is_installed},
    ffprobe::ffprobe_is_installed,
};
use ffprobe::{FfProbeError, ffprobe};
use itertools::Itertools;
use serde::{Deserialize, Serialize};

fn main() -> Result<(), Box<dyn Error>> {
    let matches = clap();
    let mut config = Defaults::open()?;

    if !ffmpeg_is_installed() || !ffprobe_is_installed() {
        panic!("not installed")
    }

    if let Some((_, defaults)) = matches.subcommand() {
        if let Some(no_audio) = defaults.get_one("no-audio") {
            config.no_audio = Some(*no_audio)
        }
        if let Some(overwrite) = defaults.get_one("overwrite") {
            config.overwrite = Some(*overwrite)
        }
        if let Some(dir) = defaults.get_one::<PathBuf>("output-directory") {
            config.out_dir = Some(dir.clone())
        }
        if let Some(dim) = defaults.get_one::<Dimensions>("dimensions") {
            if let Dimensions::Smallest = dim {
                return Err("The \"smallest\" dimensions option is currently unsupported".into());
            }
            config.dimensions = Some(dim.clone())
        }
        if let Some(file_type) = defaults.get_one::<String>("file-type") {
            config.file_type = Some(FileType::from(file_type.to_lowercase().as_str()));
        }

        match OpenOptions::new()
            .read(true)
            .write(true)
            .truncate(true)
            .open(&config.file)
        {
            Ok(file) => {
                serde_json::to_writer(file, &config)?;
                println!("Config successfully updated:\n{config:?}");
            }
            Err(e) => return Err(e.into()),
        }
        return Ok(());
    }

    let out_file = if let Some(path) = matches.get_one::<PathBuf>("output") {
        path.to_path_buf()
    } else {
        config
            .out_dir
            .map(|dir| dir.join(format!("output")))
            .unwrap_or_else(|| PathBuf::from(format!("./output")))
    };

    let ext = if let Some(ext) = out_file.extension() {
        if let Some("mp4" | "mov" | "mkv") = ext.to_str() {
            ext.to_str().unwrap()
        } else {
            return Err("Invalid output file extension".into());
        }
    } else {
        config.file_type.unwrap_or_default().as_str()
    };

    let overwrite = if let Some(n) = matches.get_one::<bool>("overwrite") {
        *n
    } else {
        config.no_audio.unwrap_or_default()
    };

    if out_file.exists() && !overwrite {
        fn ow_warning(path: &PathBuf) -> bool {
            print!(
                "WARNING: The file {} already exists. Should we replace it? [y/N]: ",
                path.display()
            );
            stdout().flush().unwrap();
            let line = std::io::stdin().lock().lines().next().unwrap().unwrap();

            match line.to_lowercase().as_str() {
                "y" => true,
                "n" | "" => false,
                _ => ow_warning(path),
            }
        }
        if !ow_warning(&out_file) {
            return Ok(());
        }
    }

    let no_audio = if let Some(n) = matches.get_one::<bool>("no-audio") {
        *n
    } else {
        config.no_audio.unwrap_or_default()
    };

    fn input(command: &mut FfmpegCommand, inputs: &mut Vec<InputFile>, path: PathBuf) {
        if path.is_dir() {
            for read_dir in path.read_dir().unwrap() {
                if let Ok(dir) = read_dir {
                    input(command, inputs, dir.path());
                }
            }
        } else {
            if path
                .extension()
                .is_some_and(|ext| ext.eq("mp4") || ext.eq("mkv") || ext.eq("mov"))
            {
                let input = ffprobe_tracks(&path).unwrap();
                // We skip "reserved" colour space because I don't know how to handle it right now
                if input.colour_space.as_str() != "reserved" {
                    command.input(path.to_str().unwrap());
                    inputs.push(input);
                }
            }
        };
    }

    let mut command = FfmpegCommand::new();
    let paths = matches.get_many::<PathBuf>("input").unwrap();
    let mut inputs = vec![];
    for path in paths {
        input(&mut command, &mut inputs, path.to_path_buf());
    }

    let (x, y) = {
        let dimensions = if let Some(d) = matches.get_one::<Dimensions>("dimensions") {
            d
        } else {
            config.dimensions.as_ref().unwrap_or(&Dimensions::Largest)
        };
        match dimensions {
            Dimensions::Largest => inputs
                .iter()
                .by_ref()
                .sorted_by(|a, b| (a.width * a.height).cmp(&(b.width * b.height)))
                .map(|i| (i.width, i.height))
                .nth(0)
                .unwrap(),
            Dimensions::Smallest => {
                // inputs.iter().by_ref().sorted_by(|a, b| {
                //     (a.width*a.height).cmp(&(b.width*b.height))
                // })
                // .map(|i| (i.width, i.height))
                // .last()
                // .unwrap()
                return Err("The \"smallest\" dimensions option is currently unsupported".into());
            }
            Dimensions::Px { x, y } => (x.clone(), y.clone()),
        }
    };

    if !no_audio {
        command.args("-f lavfi -i anullsrc=channel_layout=stereo".split(' '));
    }

    let len = inputs.len();
    if len < 2 {
        return Err("you need at least 2 videos buh".into());
    }

    let mut filter_string = String::new();
    let mut concat_string = String::new();

    for (i, input) in inputs.iter().enumerate() {
        let var = num2words::Num2Words::new(i as i32).to_words().unwrap();

        filter_string.push_str(format!("[{i}:v]scale={x}:{y}:force_original_aspect_ratio=decrease,setdar=ratio={x}/{y},setsar=sar=1/1,pad=ih*{x}/{y}/sar:ih:(ow-iw)/2:(oh-ih)/2[{var}];").as_str());
        concat_string.push_str(format!("[{var}]").as_str());
        let InputFile { audio, .. } = input;
        if !no_audio {
            concat_string.push_str(
                match audio {
                    Some(_) => format!("[{i}:a]"),
                    // to use the last audio track(our filler audio)
                    None => format!("[{len}]"),
                }
                .as_str(),
            );
        }
    }

    filter_string = format!(
        "{filter_string} {concat_string} concat=n={}:v=1{}[v]{}",
        len,
        if no_audio { "" } else { ":a=1" },
        if no_audio { "" } else { "[a]" }
    );

    let mut buf = String::new();
    let command = command
        .filter_complex(filter_string)
        .args(format!("-map [v]{}", if no_audio { "" } else { " -map [a]" }).split(' '))
        .codec_video("libx265")
        .output(out_file.with_extension(ext).to_str().unwrap())
        .overwrite();

    let debug = matches.get_one("debug");
    let mut command = if let Some(true) = debug {
        command.print_command()
    } else {
        command
    }
    .spawn()
    .unwrap();

    if let Some(true) = debug {
        command
            .take_stdout()
            .unwrap()
            .read_to_string(&mut buf)
            .unwrap();

        command
            .take_stderr()
            .unwrap()
            .read_to_string(&mut buf)
            .unwrap();

        command.wait().unwrap();
        println!("{buf}");
    } else {
        command.wait().unwrap();
    }
    Ok(())
}

#[derive(Default, Debug)]
struct InputFile {
    video: usize,
    audio: Option<usize>,
    colour_space: String,
    width: i64,
    height: i64,
}

fn ffprobe_tracks(path: impl AsRef<std::path::Path> + Debug) -> Result<InputFile, FfProbeError> {
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
            _ => continue,
        }
        if let Some(colour_space) = stream.color_space {
            input.colour_space = colour_space;
        }
        if let Some(width) = stream.width {
            input.width = width;
        }
        if let Some(height) = stream.height {
            input.height = height;
        }
    }

    Ok(input)
}

fn clap() -> ArgMatches {
    Command::new("memmet")
        .version("1.0.0")
        .author("MrDulfin")
        .about("memmet: slap some videos together, but easy")
        .arg_required_else_help(true)
        .arg(
            Arg::new("output")
                .value_parser(clap::value_parser!(PathBuf))
                .action(ArgAction::Set)
                .value_name("FILE")
                .help("The path for the output file")
                .requires("input"),
        )
        .arg(
            Arg::new("input")
                .long("input")
                .short('i')
                .value_parser(clap::value_parser!(PathBuf))
                .action(ArgAction::Append)
                .value_name("FILE or DIR")
                .help("The input files/directories to concatenate together"),
        )
        .arg(
            Arg::new("dimensions")
                .long("dimensions")
                .short('d')
                .value_parser(parse_dimensions)
                .action(ArgAction::Set)
                .help("Set the dimensions of the output video"),
        )
        .arg(
            Arg::new("no-audio")
                .long("no-audio")
                .short('n')
                .value_parser(value_parser!(bool))
                .num_args(0..=1)
                .require_equals(true)
                .default_missing_value_os("true")
                .default_missing_value("true")
                .action(ArgAction::Set)
                .help("Removes all audio from the output file"),
        )
        .arg(
            Arg::new("overwrite")
                .long("overwrite")
                .short('y')
                .value_parser(value_parser!(bool))
                .num_args(0..=1)
                .require_equals(true)
                .default_missing_value_os("true")
                .default_missing_value("true")
                .action(ArgAction::Set)
                .help("Automatically overwrites the output file if it already exists"),
        )
        .arg(
            Arg::new("debug")
                .long("debug")
                .action(ArgAction::SetTrue)
                .help("Print ffmpeg output"),
        )
        .subcommand(
            Command::new("config")
                .about("set the default input values")
                .arg(
                    Arg::new("output-directory")
                        .long("output-directory")
                        .alias("out-dir")
                        .short('o')
                        .value_parser(value_parser!(PathBuf))
                        .action(ArgAction::Set)
                        .help("Set the default output directory for your output file"),
                )
                .arg(
                    Arg::new("no-audio")
                        .long("no-audio")
                        .short('n')
                        .value_parser(value_parser!(bool)),
                )
                .arg(
                    Arg::new("overwrite")
                        .long("overwrite")
                        .short('y')
                        .value_parser(value_parser!(bool)),
                )
                .arg(
                    Arg::new("dimensions")
                        .long("dimensions")
                        .short('d')
                        .value_parser(parse_dimensions),
                )
                .arg(
                    Arg::new("file-type")
                        .long("file-type")
                        .short('f')
                        .value_parser(["mp4", "mov", "mkv"]),
                ),
        )
        .get_matches()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
enum Dimensions {
    Px { x: i64, y: i64 },
    Largest,
    Smallest,
}

fn parse_dimensions(dimensions: &str) -> Result<Dimensions, String> {
    match dimensions.to_lowercase().as_str() {
        "largest" | "l" => Ok(Dimensions::Largest),
        "smallest" | "s" => Ok(Dimensions::Smallest),
        _ => {
            let mut dimensions = dimensions.split(':');
            let x = match dimensions.nth(0) {
                Some(x) => x.parse::<i64>().map_err(|e| e.to_string())?,
                None => return Err("dimension formatted incorrectly".into()),
            };
            let y = match dimensions.nth(0) {
                Some(y) => y.parse::<i64>().map_err(|e| e.to_string())?,
                None => return Err("dimension formatted incorrectly".into()),
            };
            Ok(Dimensions::Px { x, y })
        }
    }
}

#[derive(Default, Debug, Deserialize, Serialize)]
struct Defaults {
    out_dir: Option<PathBuf>,
    dimensions: Option<Dimensions>,
    no_audio: Option<bool>,
    overwrite: Option<bool>,
    file_type: Option<FileType>,
    #[serde(skip)]
    file: PathBuf,
}

#[derive(Default, Debug, Deserialize, Serialize)]
enum FileType {
    #[default]
    Mp4,
    Mov,
    Mkv,
}

impl FileType {
    fn as_str(&self) -> &'static str {
        match self {
            FileType::Mp4 => "mp4",
            FileType::Mov => "mov",
            FileType::Mkv => "mkv",
        }
    }
}

impl From<&str> for FileType {
    fn from(value: &str) -> Self {
        match value {
            "mp4" => FileType::Mp4,
            "mov" => FileType::Mov,
            "mkv" => FileType::Mkv,
            _ => unreachable!(),
        }
    }
}

impl Defaults {
    fn open() -> Result<Self, Box<dyn Error>> {
        if let Some(dirs) = directories::ProjectDirs::from("moe", "mrdulfin", "memmet") {
            let config_dir = dirs.config_dir();

            fs::create_dir_all(config_dir).or_else(|err| {
                if err.kind() == std::io::ErrorKind::AlreadyExists {
                    Ok(())
                } else {
                    Err(err)
                }
            })?;

            let path = config_dir.join("config");
            if let Ok(file) = OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .open(path.clone())
            {
                serde_json::from_reader(&file)
                    .or_else(|e| {
                        if e.is_eof() {
                            Ok(Defaults::default())
                        } else {
                            Err(e.into())
                        }
                    })
                    .map(|mut d| {
                        d.file = path;
                        d
                    })
            } else {
                Ok(Defaults::default())
            }
        } else {
            todo!()
        }
    }
}
