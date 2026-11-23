use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    FromSample, Sample, SampleFormat, SizedSample, StreamConfig,
};
use songbird::input::{core::io::MediaSource, Input, RawAdapter};
use std::{
    collections::VecDeque,
    io::{self, Read, Seek, SeekFrom},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
};

const CAP: usize = 48_000 / 5 * 2;
type Queue = Arc<Mutex<VecDeque<f32>>>;
#[derive(Clone)]
pub struct Bus {
    mic: Queue,
    output: Queue,
    pub mute: Arc<AtomicBool>,
    pub deaf: Arc<AtomicBool>,
    pub failed: Arc<AtomicBool>,
}
impl Bus {
    pub fn new() -> Self {
        Self {
            mic: Arc::new(Mutex::new(VecDeque::with_capacity(CAP))),
            output: Arc::new(Mutex::new(VecDeque::with_capacity(CAP))),
            mute: Arc::new(AtomicBool::new(false)),
            deaf: Arc::new(AtomicBool::new(false)),
            failed: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn input(&self) -> Input {
        RawAdapter::new(
            Microphone {
                bus: self.clone(),
                pending: VecDeque::new(),
            },
            48_000,
            2,
        )
        .into()
    }
    pub fn mix(&self, sources: impl Iterator<Item = Vec<i16>>) {
        if self.deaf.load(Ordering::Relaxed) {
            return;
        }
        let mut mix = vec![0.0_f32; 1920];
        for source in sources {
            for (out, sample) in mix.iter_mut().zip(source) {
                *out += sample as f32 / 32768.0;
            }
        }
        if let Ok(mut queue) = self.output.try_lock() {
            for sample in mix {
                push(&mut queue, sample.clamp(-1.0, 1.0));
            }
        }
    }
    pub fn clear(&self) {
        if let Ok(mut q) = self.mic.lock() {
            q.clear();
        }
        if let Ok(mut q) = self.output.lock() {
            q.clear();
        }
    }
    pub fn controls(&self, mute: bool, deaf: bool) {
        self.mute.store(mute || deaf, Ordering::Relaxed);
        self.deaf.store(deaf, Ordering::Relaxed);
        if mute || deaf {
            if let Ok(mut q) = self.mic.lock() {
                q.clear();
            }
        }
        if deaf {
            if let Ok(mut q) = self.output.lock() {
                q.clear();
            }
        }
    }
}
fn push(queue: &mut VecDeque<f32>, sample: f32) {
    if queue.len() == CAP {
        queue.pop_front();
    }
    queue.push_back(sample);
}
pub struct Audio {
    stop: Option<mpsc::SyncSender<()>>,
}
impl Audio {
    pub async fn start(bus: Bus) -> Result<Self, String> {
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (stop, stopped) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let streams = streams(&bus);
            match streams {
                Ok((_input, _output)) => {
                    let _ = ready_tx.send(Ok(()));
                    let _ = stopped.recv();
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                }
            }
            bus.clear();
        });
        ready_rx
            .await
            .map_err(|_| "Audio worker stopped".to_owned())??;
        Ok(Self { stop: Some(stop) })
    }
}
impl Drop for Audio {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.try_send(());
        }
    }
}
fn streams(bus: &Bus) -> Result<(cpal::Stream, cpal::Stream), String> {
    let host = cpal::default_host();
    let input = host.default_input_device().ok_or("No default microphone")?;
    let output = host.default_output_device().ok_or("No default speakers")?;
    let ic = input
        .default_input_config()
        .map_err(|_| "Microphone configuration unavailable")?;
    let oc = output
        .default_output_config()
        .map_err(|_| "Speaker configuration unavailable")?;
    let input = match ic.sample_format() {
        SampleFormat::F32 => capture::<f32>(&input, ic.into(), bus.clone()),
        SampleFormat::I16 => capture::<i16>(&input, ic.into(), bus.clone()),
        SampleFormat::U16 => capture::<u16>(&input, ic.into(), bus.clone()),
        SampleFormat::I32 => capture::<i32>(&input, ic.into(), bus.clone()),
        _ => return Err("Unsupported microphone sample format".into()),
    }
    .map_err(|_| "Cannot open microphone; check OS microphone permission")?;
    let output = match oc.sample_format() {
        SampleFormat::F32 => playback::<f32>(&output, oc.into(), bus.clone()),
        SampleFormat::I16 => playback::<i16>(&output, oc.into(), bus.clone()),
        SampleFormat::U16 => playback::<u16>(&output, oc.into(), bus.clone()),
        SampleFormat::I32 => playback::<i32>(&output, oc.into(), bus.clone()),
        _ => return Err("Unsupported speaker sample format".into()),
    }
    .map_err(|_| "Cannot open speakers")?;
    input.play().map_err(|_| "Cannot start microphone")?;
    output.play().map_err(|_| "Cannot start speakers")?;
    Ok((input, output))
}
fn capture<T: SizedSample + Sample>(
    device: &cpal::Device,
    config: StreamConfig,
    bus: Bus,
) -> Result<cpal::Stream, cpal::Error>
where
    f32: FromSample<T>,
{
    let channels = config.channels as usize;
    let rate = config.sample_rate as f64;
    let mut phase = 0.0;
    let failed = bus.failed.clone();
    device.build_input_stream(
        config,
        move |samples: &[T], _| {
            if bus.mute.load(Ordering::Relaxed) || bus.deaf.load(Ordering::Relaxed) {
                return;
            }
            if let Ok(mut queue) = bus.mic.try_lock() {
                for frame in samples.chunks_exact(channels) {
                    phase += 48_000.0;
                    while phase >= rate {
                        phase -= rate;
                        push(&mut queue, frame[0].to_sample::<f32>());
                        push(
                            &mut queue,
                            frame[if channels > 1 { 1 } else { 0 }].to_sample::<f32>(),
                        );
                    }
                }
            }
        },
        move |_| {
            failed.store(true, Ordering::Relaxed);
        },
        None,
    )
}
fn playback<T: SizedSample + Sample + FromSample<f32>>(
    device: &cpal::Device,
    config: StreamConfig,
    bus: Bus,
) -> Result<cpal::Stream, cpal::Error> {
    let channels = config.channels as usize;
    let step = 48_000.0 / config.sample_rate as f64;
    let mut phase = 1.0;
    let mut current = [0.0_f32; 2];
    let failed = bus.failed.clone();
    device.build_output_stream(
        config,
        move |samples: &mut [T], _| {
            samples.fill(T::from_sample(0.0));
            if let Ok(mut queue) = bus.output.try_lock() {
                if bus.deaf.load(Ordering::Relaxed) {
                    queue.clear();
                    current = [0.0; 2];
                    return;
                }
                for frame in samples.chunks_exact_mut(channels) {
                    while phase >= 1.0 {
                        phase -= 1.0;
                        current = [
                            queue.pop_front().unwrap_or(0.0),
                            queue.pop_front().unwrap_or(0.0),
                        ];
                    }
                    for (i, sample) in frame.iter_mut().enumerate() {
                        *sample = T::from_sample(if channels == 1 {
                            (current[0] + current[1]) * 0.5
                        } else {
                            current[i.min(1)]
                        });
                    }
                    phase += step;
                }
            }
        },
        move |_| {
            failed.store(true, Ordering::Relaxed);
        },
        None,
    )
}
struct Microphone {
    bus: Bus,
    pending: VecDeque<u8>,
}
impl Read for Microphone {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let mut queue = self
            .bus
            .mic
            .lock()
            .map_err(|_| io::Error::other("Audio queue poisoned"))?;
        if self.bus.mute.load(Ordering::Relaxed) || self.bus.deaf.load(Ordering::Relaxed) {
            queue.clear();
            self.pending.clear();
        }
        for byte in bytes.iter_mut() {
            if self.pending.is_empty() {
                self.pending
                    .extend(queue.pop_front().unwrap_or(0.0).to_le_bytes());
            }
            *byte = self.pending.pop_front().unwrap_or(0);
        }
        Ok(bytes.len())
    }
}
impl Seek for Microphone {
    fn seek(&mut self, _: SeekFrom) -> io::Result<u64> {
        Err(io::ErrorKind::Unsupported.into())
    }
}
impl MediaSource for Microphone {
    fn is_seekable(&self) -> bool {
        false
    }
    fn byte_len(&self) -> Option<u64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use songbird::input::{
        codecs::{get_codec_registry, get_probe},
        LiveInput,
    };
    use symphonia::core::audio::Signal;

    #[test]
    fn microphone_input_promotes_and_decodes_pcm() {
        for sample in [0.0, 0.25] {
            let bus = Bus::new();
            bus.mic
                .lock()
                .unwrap()
                .extend(std::iter::repeat_n(sample, CAP));
            let Input::Live(input, _) = bus.input() else {
                panic!("microphone must be live")
            };
            let LiveInput::Parsed(mut parsed) =
                input.promote(get_codec_registry(), get_probe()).unwrap()
            else {
                panic!("microphone must be playable")
            };
            let packet = parsed.format.next_packet().unwrap();
            let decoded = parsed.decoder.decode(&packet).unwrap();
            assert_eq!(decoded.spec().rate, 48_000);
            assert_eq!(decoded.spec().channels.count(), 2);
            let symphonia::core::audio::AudioBufferRef::F32(buffer) = decoded else {
                panic!("expected f32")
            };
            assert!(buffer.frames() > 0);
            assert!(buffer.chan(0).iter().all(|value| *value == sample));
            assert!(buffer.chan(1).iter().all(|value| *value == sample));
        }
    }
    #[test]
    fn bounded_mix_clips_and_deafen_discards() {
        let bus = Bus::new();
        for _ in 0..20 {
            bus.mix([vec![32767; 1920], vec![32767; 1920]].into_iter());
        }
        let q = bus.output.lock().unwrap();
        assert_eq!(q.len(), CAP);
        assert!(q.iter().all(|s| *s == 1.0));
        drop(q);
        bus.clear();
        bus.deaf.store(true, Ordering::Relaxed);
        bus.mix([vec![1; 1920]].into_iter());
        assert!(bus.output.lock().unwrap().is_empty());
    }
    #[test]
    fn microphone_silence_and_mute() {
        let bus = Bus::new();
        let mut mic = Microphone {
            bus: bus.clone(),
            pending: VecDeque::new(),
        };
        let mut bytes = [0; 8];
        assert_eq!(mic.read(&mut bytes).unwrap(), 8);
        assert_eq!(bytes, [0; 8]);
        bus.mic.lock().unwrap().push_back(0.5);
        bus.mute.store(true, Ordering::Relaxed);
        mic.read_exact(&mut bytes).unwrap();
        assert_eq!(bytes, [0; 8]);
    }
    #[test]
    fn mic_data_controls_and_bounded_queue() {
        let bus = Bus::new();
        {
            let mut q = bus.mic.lock().unwrap();
            for _ in 0..CAP + 100 {
                push(&mut q, 0.25);
            }
            assert_eq!(q.len(), CAP);
        }
        let mut mic = Microphone {
            bus: bus.clone(),
            pending: VecDeque::new(),
        };
        let mut bytes = [0; 4];
        mic.read_exact(&mut bytes).unwrap();
        assert_eq!(f32::from_le_bytes(bytes), 0.25);
        bus.mix([vec![100; 1920]].into_iter());
        bus.controls(true, false);
        assert!(bus.mic.lock().unwrap().is_empty());
        assert!(!bus.output.lock().unwrap().is_empty());
        bus.controls(false, true);
        assert!(bus.mute.load(Ordering::Relaxed));
        assert!(bus.output.lock().unwrap().is_empty());
        assert!(mic.seek(SeekFrom::Start(0)).is_err());
    }
}
