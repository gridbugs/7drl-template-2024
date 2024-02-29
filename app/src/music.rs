use currawong::prelude::*;
use rand::{rngs::StdRng, Rng, SeedableRng};

const C_MAJOR_SCALE: &[NoteName] = &[
    NoteName::A,
    NoteName::B,
    NoteName::C,
    NoteName::D,
    NoteName::E,
    NoteName::F,
    NoteName::G,
];

fn make_scale_base_freqs(note_names: &[NoteName]) -> Vec<Sfreq> {
    note_names
        .into_iter()
        .map(|&name| const_(name.in_octave(OCTAVE_0).freq()))
        .collect()
}

fn random_note_c_major(base_hz: Sf64, range_hz: Sf64) -> Sfreq {
    sfreq_hz(base_hz + (noise_01() * range_hz))
        .filter(quantize_to_scale(make_scale_base_freqs(C_MAJOR_SCALE)).build())
}

fn super_saw_osc(freq_hz: Sf64, detune: Sf64, n: usize, gate: Gate) -> Sf64 {
    let trigger = gate.to_trigger_rising_edge();
    (0..n)
        .map(|i| {
            let delta_hz = ((i + 1) as f64 * &detune) / n as f64;
            let osc = oscillator_hz(Waveform::Saw, &freq_hz * (1 + &delta_hz))
                .reset_trigger(&trigger)
                .build()
                + oscillator_hz(Waveform::Saw, &freq_hz * (1 - &delta_hz))
                    .reset_trigger(&trigger)
                    .build();
            osc / (i as f64 + 1.0)
        })
        .sum::<Sf64>()
        + oscillator_hz(Waveform::Saw, &freq_hz).build()
}

fn voice(freq: Sfreq, gate: Gate) -> Sf64 {
    let freq_hz = freq.hz();
    let osc = super_saw_osc(freq_hz.clone(), const_(0.01), 1, gate.clone())
        + super_saw_osc(freq_hz.clone() * 2.0, const_(0.01), 1, gate.clone())
        + super_saw_osc(freq_hz.clone() * 1.25, const_(0.01), 1, gate.clone());
    let env_amp = adsr_linear_01(&gate).attack_s(0.1).release_s(8.0).build();
    let env_lpf = adsr_linear_01(&gate)
        .attack_s(0.1)
        .release_s(4.0)
        .build()
        .exp_01(1.0);
    (osc.filter(low_pass_moog_ladder(1000 * &env_lpf).resonance(2.0).build())
        + osc.filter(low_pass_moog_ladder(10000 * &env_lpf).build()))
    .mul_lazy(&env_amp)
}

fn random_replace_loop(
    trigger: Trigger,
    anchor: Sfreq,
    palette: Sfreq,
    length: usize,
    replace_probability_01: Sf64,
    anchor_probability_01: Sf64,
) -> Sfreq {
    let mut rng = StdRng::from_entropy();
    let mut sequence: Vec<Option<Freq>> = vec![None; length];
    let mut index = 0;
    let mut anchor_on_0 = false;
    let mut first_note = true;
    Signal::from_fn_mut(move |ctx| {
        let trigger = trigger.sample(ctx);
        if trigger {
            if rng.gen::<f64>() < replace_probability_01.sample(ctx) {
                sequence[index] = Some(palette.sample(ctx));
            }
            if index == 0 {
                anchor_on_0 = rng.gen::<f64>() < anchor_probability_01.sample(ctx);
            }
        }
        let freq = if first_note {
            first_note = false;
            anchor.sample(ctx)
        } else if anchor_on_0 && index == 0 {
            anchor.sample(ctx)
        } else if let Some(freq) = sequence[index] {
            freq
        } else {
            let freq = palette.sample(ctx);
            sequence[index] = Some(freq);
            freq
        };
        if trigger {
            index = (index + 1) % sequence.len();
        }
        freq
    })
}

fn synth_signal(trigger: Trigger) -> Sf64 {
    let freq = random_replace_loop(
        trigger.clone(),
        const_(NoteName::A.in_octave(OCTAVE_1).freq()),
        random_note_c_major(const_(80.0), const_(200.0)),
        4,
        const_(0.1),
        const_(0.5),
    );
    let gate = trigger.to_gate_with_duration_s(0.1);
    let modulate = 1.0
        - oscillator_s(Waveform::Triangle, 60.0)
            .build()
            .signed_to_01();
    let lfo = oscillator_hz(Waveform::Sine, &modulate * 8.0).build();
    voice(freq, gate)
        .filter(down_sample(1.0 + &modulate * 10.0).build())
        .filter(low_pass_moog_ladder(10000.0 + &lfo * 2000.0).build())
        .filter(
            compress()
                .threshold(2.0)
                .scale(1.0 + &modulate * 2.0)
                .ratio(0.1)
                .build(),
        )
        .filter(high_pass_butterworth(10.0).build())
}

fn drum_signal(trigger: Trigger) -> Sf64 {
    const HAT_CLOSED: usize = 0;
    const SNARE: usize = 1;
    const KICK: usize = 2;
    let drum_pattern = {
        let hat_closed = 1 << HAT_CLOSED;
        let snare = 1 << SNARE;
        let kick = 1 << KICK;
        vec![
            hat_closed | kick,
            hat_closed,
            hat_closed | snare,
            hat_closed,
            hat_closed | kick,
            hat_closed,
            hat_closed | snare,
            hat_closed,
            hat_closed | kick,
            hat_closed,
            hat_closed | snare,
            hat_closed,
            hat_closed | kick,
            hat_closed | kick,
            hat_closed | snare,
            hat_closed,
        ]
    };
    let drum_sequence = bitwise_pattern_triggers_8(trigger, drum_pattern).triggers;
    match &drum_sequence.as_slice() {
        &[hat_closed_trigger, snare_trigger, kick_trigger, ..] => {
            hat_closed(hat_closed_trigger.clone()).build()
                + snare(snare_trigger.clone()).build()
                + kick(kick_trigger.clone()).build()
        }
        _ => panic!(),
    }
}

pub fn signal() -> Sf64 {
    let trigger = periodic_trigger_hz(4.0).build();
    (synth_signal(trigger.divide(16)) + drum_signal(trigger.divide(1))) * 0.2
}
