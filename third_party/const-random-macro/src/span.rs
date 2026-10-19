use proc_macro::Span;
use std::option_env;

use once_cell::race::OnceBox;
use sha2::{Digest, Sha256};


static SEED: OnceBox<Vec<u8>> = OnceBox::new();

fn get_seed() -> &'static [u8] {
    &SEED.get_or_init(|| {
        if let Some(value) = option_env!("CONST_RANDOM_SEED") {
 	    Box::new(value.as_bytes().to_vec())
    	} else {
            let mut value = [0u8; 32];
            getrandom::getrandom(&mut value).unwrap();
            Box::new(value.to_vec())
        }
    })[..]
}

pub(crate) fn gen_random<T: Random>() -> T {
    Random::random()
}

pub(crate) fn gen_random_bytes(output: &mut [u8]) {
    squeeze(output)
}

pub(crate) trait Random {
    fn random() -> Self;
}

/// Fills `output` from SHA-256 in counter mode keyed by the seed and the call site's span: the
/// same call site always expands to the same bytes within one build, different call sites to
/// unrelated ones. (The upstream crate used the SHAKE256 extendable-output function of
/// `tiny-keccak`, which is CC0-1.0 licensed; this vendored copy has no such dependency.)
fn squeeze(output: &mut [u8]) {
    let span = format!("{:?}", Span::call_site());
    for (counter, chunk) in output.chunks_mut(32).enumerate() {
        let mut hasher = Sha256::new();
        hasher.update(get_seed());
        hasher.update(span.as_bytes());
        hasher.update((counter as u64).to_le_bytes());
        chunk.copy_from_slice(&hasher.finalize()[..chunk.len()]);
    }
}

impl Random for u64 {
    fn random() -> Self {
        let mut output = [0; 8];
        squeeze(&mut output);
        Self::from_ne_bytes(output)
    }
}

impl Random for u128 {
    fn random() -> Self {
        let mut output = [0; 16];
        squeeze(&mut output);
        Self::from_ne_bytes(output)
    }
}

impl Random for u8 {
    fn random() -> Self {
        u64::random() as u8
    }
}

impl Random for u16 {
    fn random() -> Self {
        u64::random() as u16
    }
}

impl Random for u32 {
    fn random() -> Self {
        u64::random() as u32
    }
}

impl Random for i8 {
    fn random() -> Self {
        i64::random() as i8
    }
}

impl Random for i16 {
    fn random() -> Self {
        i64::random() as i16
    }
}

impl Random for i32 {
    fn random() -> Self {
        i64::random() as i32
    }
}

impl Random for i64 {
    fn random() -> Self {
        u64::random() as i64
    }
}

impl Random for i128 {
    fn random() -> Self {
        u128::random() as i128
    }
}
