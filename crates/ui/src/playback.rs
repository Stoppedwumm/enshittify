//! Transport: play/pause, J/K/L shuttle, seeking. At 1× forward the audio
//! clock is the master; other speeds run off the wall clock without audio.

use std::sync::Arc;
use std::time::Instant;

use cutroom_audio::AudioEngine;
use cutroom_audio::timeline::PathResolver;
use cutroom_core::model::DocSnapshot;
use cutroom_core::{SeqId, Ticks};
use cutroom_media::MediaBackend;
use loro::Frontiers;

#[derive(Debug)]
enum State {
    Paused,
    Playing {
        speed: i32,
        anchor_tick: Ticks,
        anchor_time: Instant,
        version: Frontiers,
    },
}

pub struct Playback {
    pub playhead: Ticks,
    state: State,
    audio: AudioEngine,
}

impl Playback {
    pub fn new(backend: Arc<dyn MediaBackend>, use_device: bool) -> Self {
        Playback {
            playhead: Ticks::ZERO,
            state: State::Paused,
            audio: AudioEngine::new(backend, use_device),
        }
    }

    pub fn is_playing(&self) -> bool {
        matches!(self.state, State::Playing { .. })
    }

    pub fn speed(&self) -> i32 {
        match self.state {
            State::Playing { speed, .. } => speed,
            State::Paused => 0,
        }
    }

    pub fn audio_device(&self) -> &str {
        self.audio.device_name()
    }

    fn start(
        &mut self,
        speed: i32,
        snap: &Arc<DocSnapshot>,
        seq: SeqId,
        resolve: &PathResolver,
        version: Frontiers,
    ) {
        self.audio.stop();
        if speed == 1 {
            self.audio
                .play(snap.clone(), seq, self.playhead, resolve.clone());
        }
        self.state = State::Playing {
            speed,
            anchor_tick: self.playhead,
            anchor_time: Instant::now(),
            version,
        };
    }

    pub fn play(
        &mut self,
        snap: &Arc<DocSnapshot>,
        seq: SeqId,
        resolve: &PathResolver,
        version: Frontiers,
    ) {
        if snap
            .main()
            .map(|s| self.playhead >= s.duration())
            .unwrap_or(false)
        {
            self.playhead = Ticks::ZERO;
        }
        self.start(1, snap, seq, resolve, version);
    }

    pub fn pause(&mut self) {
        self.audio.stop();
        self.state = State::Paused;
    }

    pub fn toggle(
        &mut self,
        snap: &Arc<DocSnapshot>,
        seq: SeqId,
        resolve: &PathResolver,
        version: Frontiers,
    ) {
        if self.is_playing() {
            self.pause();
        } else {
            self.play(snap, seq, resolve, version);
        }
    }

    /// J (dir = -1) / L (dir = +1): start, or double the speed up to 4×.
    pub fn shuttle(
        &mut self,
        dir: i32,
        snap: &Arc<DocSnapshot>,
        seq: SeqId,
        resolve: &PathResolver,
        version: Frontiers,
    ) {
        let cur = self.speed();
        let next = if cur.signum() == dir.signum() && cur != 0 {
            (cur * 2).clamp(-4, 4)
        } else {
            dir
        };
        self.start(next, snap, seq, resolve, version);
    }

    /// Moves the playhead; keeps playing from the new position.
    pub fn seek(
        &mut self,
        t: Ticks,
        snap: &Arc<DocSnapshot>,
        seq: SeqId,
        resolve: &PathResolver,
        version: Frontiers,
    ) {
        self.playhead = t.max(Ticks::ZERO);
        if let State::Playing { speed, .. } = self.state {
            self.start(speed, snap, seq, resolve, version);
        }
    }

    /// Seek without restarting playback (paused scrubbing).
    pub fn set_playhead(&mut self, t: Ticks) {
        self.playhead = t.max(Ticks::ZERO);
    }

    /// Advances the playhead. Restarts audio when the document changed so
    /// edits are heard immediately.
    pub fn update(
        &mut self,
        snap: &Arc<DocSnapshot>,
        seq: SeqId,
        resolve: &PathResolver,
        version: Frontiers,
        end: Ticks,
    ) {
        let State::Playing {
            speed,
            anchor_tick,
            anchor_time,
            version: v,
        } = &self.state
        else {
            return;
        };
        let (speed, anchor_tick, anchor_time) = (*speed, *anchor_tick, *anchor_time);
        if *v != version {
            self.start(speed, snap, seq, resolve, version);
            return;
        }
        let t = match (speed, self.audio.position()) {
            (1, Some(p)) => p,
            _ => {
                let dt = anchor_time.elapsed().as_secs_f64() * speed as f64;
                anchor_tick + Ticks::from_secs_f64(dt)
            }
        };
        if t >= end && speed > 0 {
            self.playhead = end;
            self.pause();
        } else if t <= Ticks::ZERO && speed < 0 {
            self.playhead = Ticks::ZERO;
            self.pause();
        } else {
            self.playhead = t;
        }
    }
}
