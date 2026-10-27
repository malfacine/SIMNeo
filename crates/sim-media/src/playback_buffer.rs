//! Pré-leitura limitada de pacotes comprimidos; exclusiva da reprodução.
use super::*;
use std::{collections::VecDeque, sync::Condvar};

const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_PACKETS: usize = 2048;
const WAIT: Duration = Duration::from_millis(10);

#[derive(Default)]
struct State {
    packets: VecDeque<ffmpeg::Packet>,
    bytes: usize,
    end: Option<Result<(), Error>>,
}
struct Queue {
    state: Mutex<State>,
    changed: Condvar,
    time_base: f64,
    speed: u32,
}
impl Queue {
    fn span(&self, state: &State) -> u64 {
        let stamp = |packet: &ffmpeg::Packet| packet.dts().or(packet.pts());
        match (
            state.packets.front().and_then(stamp),
            state.packets.back().and_then(stamp),
        ) {
            (Some(first), Some(last)) => {
                ((last.saturating_sub(first).max(0) as f64) * self.time_base * 1000.0) as u64
            }
            _ => 0,
        }
    }
    fn push(&self, packet: ffmpeg::Packet, cancelled: &AtomicBool, stop: &AtomicBool) -> bool {
        if packet.size() > MAX_BYTES {
            self.finish(Err(Error::Frame));
            return false;
        }
        let mut state = self.state.lock().unwrap();
        while state.bytes + packet.size() > MAX_BYTES
            || state.packets.len() >= MAX_PACKETS
            || self.span(&state) >= 4000 * u64::from(self.speed)
        {
            if cancelled.load(Ordering::Acquire) || stop.load(Ordering::Acquire) {
                return false;
            }
            state = self.changed.wait_timeout(state, WAIT).unwrap().0;
        }
        if cancelled.load(Ordering::Acquire) || stop.load(Ordering::Acquire) {
            return false;
        }
        state.bytes += packet.size();
        state.packets.push_back(packet);
        self.changed.notify_all();
        true
    }
    fn finish(&self, result: Result<(), Error>) {
        self.state.lock().unwrap().end = Some(result);
        self.changed.notify_all();
    }
}

pub(super) struct Reader {
    queue: Arc<Queue>,
    stop: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    prefill: bool,
}
impl Reader {
    pub fn start(
        mut input: Input,
        index: usize,
        time_base: f64,
        speed: u32,
        cancelled: Arc<AtomicBool>,
    ) -> Self {
        let queue = Arc::new(Queue {
            state: Default::default(),
            changed: Condvar::new(),
            time_base,
            speed,
        });
        let stop = input.interrupt.read_stop.clone();
        let producer = queue.clone();
        let cancel = cancelled.clone();
        let stopped = stop.clone();
        let worker = std::thread::spawn(move || {
            loop {
                if cancel.load(Ordering::Acquire) || stopped.load(Ordering::Acquire) {
                    return;
                }
                input.interrupt.arm();
                let mut packet = ffmpeg::Packet::empty();
                match packet.read(&mut input.context) {
                    Ok(()) => {
                        if packet.stream() == index && !producer.push(packet, &cancel, &stopped) {
                            return;
                        }
                    }
                    Err(ffmpeg::Error::Eof) => {
                        producer.finish(Ok(()));
                        return;
                    }
                    Err(error) => {
                        producer.finish(Err(error.into()));
                        return;
                    }
                }
            }
        });
        Self {
            queue,
            stop,
            cancelled,
            worker: Some(worker),
            prefill: true,
        }
    }
    pub fn next(
        &mut self,
        control: &archive::PlaybackControl,
        clock: &mut archive::PlaybackClock,
    ) -> Result<Option<ffmpeg::Packet>, Error> {
        let started = Instant::now();
        let mut waited = Duration::ZERO;
        let mut state = self.queue.state.lock().unwrap();
        if state.packets.is_empty() && state.end.is_none() {
            self.prefill = true;
        }
        loop {
            if self.cancelled.load(Ordering::Acquire) {
                control.buffering.store(false, Ordering::Release);
                return Ok(None);
            }
            let full = state.bytes >= MAX_BYTES / 2 || state.packets.len() >= MAX_PACKETS / 2;
            let ready = !self.prefill
                || self.queue.span(&state) >= 1000 * u64::from(self.queue.speed)
                || full
                || state.end.is_some()
                || started.elapsed() >= Duration::from_secs(1);
            if ready && let Some(packet) = state.packets.pop_front() {
                self.prefill = false;
                state.bytes -= packet.size();
                self.queue.changed.notify_all();
                clock.exclude(waited);
                control.buffering.store(false, Ordering::Release);
                return Ok(Some(packet));
            }
            if let Some(result) = state.end.clone() {
                clock.exclude(waited);
                control.buffering.store(false, Ordering::Release);
                return result.map(|()| None);
            }
            control.buffering.store(true, Ordering::Release);
            let before = Instant::now();
            state = self.queue.changed.wait_timeout(state, WAIT).unwrap().0;
            waited += before.elapsed();
        }
    }
}
impl Drop for Reader {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.queue.changed.notify_all();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queue_preserves_packets_and_backpressure_respects_memory_limit() {
        let queue = Arc::new(Queue {
            state: Default::default(),
            changed: Condvar::new(),
            time_base: 0.001,
            speed: 16,
        });
        let cancelled = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let mut first = ffmpeg::Packet::new(MAX_BYTES);
        first.set_dts(Some(100));
        assert!(queue.push(first, &cancelled, &stop));
        let q = queue.clone();
        let c = cancelled.clone();
        let s = stop.clone();
        let producer = std::thread::spawn(move || q.push(ffmpeg::Packet::new(10), &c, &s));
        std::thread::sleep(Duration::from_millis(30));
        assert!(!producer.is_finished());
        {
            let mut state = queue.state.lock().unwrap();
            assert_eq!(state.bytes, MAX_BYTES);
            assert_eq!(state.packets.pop_front().unwrap().dts(), Some(100));
            state.bytes = 0;
            queue.changed.notify_all();
        }
        assert!(producer.join().unwrap());
        assert_eq!(queue.state.lock().unwrap().bytes, 10);
    }
    #[test]
    fn stopping_releases_a_producer_blocked_by_a_full_queue() {
        let queue = Arc::new(Queue {
            state: Default::default(),
            changed: Condvar::new(),
            time_base: 0.001,
            speed: 16,
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        assert!(queue.push(ffmpeg::Packet::new(MAX_BYTES), &cancel, &stop));
        let q = queue.clone();
        let c = cancel.clone();
        let s = stop.clone();
        let worker = std::thread::spawn(move || {
            q.push(ffmpeg::Packet::new(1), &c, &s);
        });
        let reader = Reader {
            queue,
            stop,
            cancelled: cancel,
            worker: Some(worker),
            prefill: true,
        };
        let now = Instant::now();
        drop(reader);
        assert!(now.elapsed() < Duration::from_secs(1));
    }
    #[test]
    fn short_clip_drains_before_end_and_network_wait_does_not_advance_clock() {
        let queue = Arc::new(Queue {
            state: Default::default(),
            changed: Condvar::new(),
            time_base: 0.001,
            speed: 16,
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let q = queue.clone();
        let c = cancel.clone();
        let s = stop.clone();
        let worker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            let mut packet = ffmpeg::Packet::new(5);
            packet.set_dts(Some(0));
            q.push(packet, &c, &s);
            q.finish(Ok(()));
        });
        let mut reader = Reader {
            queue,
            stop,
            cancelled: cancel,
            worker: Some(worker),
            prefill: true,
        };
        let control = archive::PlaybackControl::default();
        let mut clock = archive::PlaybackClock::default();
        assert_eq!(
            reader.next(&control, &mut clock).unwrap().unwrap().size(),
            5
        );
        let now = Instant::now();
        assert!(control.pace(60, &mut clock, &reader.cancelled));
        assert!(now.elapsed() >= Duration::from_millis(40));
        assert!(reader.next(&control, &mut clock).unwrap().is_none());
        assert!(!control.buffering.load(Ordering::Acquire));
    }
    #[test]
    fn read_ahead_duration_tracks_the_selected_playback_speed() {
        for speed in [1, 16] {
            let queue = Arc::new(Queue {
                state: Default::default(),
                changed: Condvar::new(),
                time_base: 0.001,
                speed,
            });
            let cancel = Arc::new(AtomicBool::new(false));
            let stop = Arc::new(AtomicBool::new(false));
            for stamp in [0, 4000 * i64::from(speed)] {
                let mut packet = ffmpeg::Packet::new(1);
                packet.set_dts(Some(stamp));
                assert!(queue.push(packet, &cancel, &stop));
            }
            let q = queue.clone();
            let c = cancel.clone();
            let s = stop.clone();
            let worker = std::thread::spawn(move || q.push(ffmpeg::Packet::new(1), &c, &s));
            std::thread::sleep(Duration::from_millis(30));
            assert!(!worker.is_finished());
            {
                let mut state = queue.state.lock().unwrap();
                state.packets.pop_front();
                state.bytes -= 1;
                queue.changed.notify_all();
            }
            assert!(worker.join().unwrap());
        }
    }
}
