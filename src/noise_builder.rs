use std::str::{FromStr, SplitWhitespace};

use noise::{
    Abs, Add, Checkerboard, Clamp, Constant, Max, Min, Multiply, Negate, NoiseFn, Perlin, Power,
    ScalePoint, Simplex,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]

/// Tree of noise functions that function like expressions taking eachother as inputs
/// Every function takes the z and x position as inputs by default
pub enum NoiseBuilder {
    Constant(f64),
    Abs(Box<NoiseBuilder>),
    Neg(Box<NoiseBuilder>),
    Min(Box<NoiseBuilder>, Box<NoiseBuilder>),
    Max(Box<NoiseBuilder>, Box<NoiseBuilder>),
    Add(Box<NoiseBuilder>, Box<NoiseBuilder>),
    Mul(Box<NoiseBuilder>, Box<NoiseBuilder>),
    Pow(Box<NoiseBuilder>, Box<NoiseBuilder>),
    PowI(i32, Box<NoiseBuilder>),
    ScaleInput(f64, f64, Box<NoiseBuilder>),
    Clamp(f64, f64, Box<NoiseBuilder>),
    Checkerboard,
    /// argument is seed
    Perlin(u32),
    /// argument is seed
    Simplex(u32),
}

impl NoiseBuilder {
    pub fn build(self) -> DynNoise {
        match self {
            NoiseBuilder::Constant(v) => dynn(Constant::new(v)),
            NoiseBuilder::Abs(builder) => dynn(Abs::new(builder.build())),
            NoiseBuilder::Neg(builder) => dynn(Negate::new(builder.build())),
            // i try to do some optimization for constants, buts its a bit messy
            NoiseBuilder::Add(builder_a, builder_b) => match *builder_a {
                NoiseBuilder::Constant(v) => dynn(Add::new(Constant::new(v), builder_b.build())),
                _ => dynn(Add::new(builder_a.build(), builder_b.build())),
            },
            NoiseBuilder::Mul(builder_a, builder_b) => match *builder_a {
                NoiseBuilder::Constant(v) => {
                    dynn(Multiply::new(Constant::new(v), builder_b.build()))
                }
                _ => dynn(Multiply::new(builder_a.build(), builder_b.build())),
            },
            NoiseBuilder::Min(builder_a, builder_b) => match *builder_a {
                NoiseBuilder::Constant(v) => dynn(Min::new(Constant::new(v), builder_b.build())),
                _ => dynn(Min::new(builder_a.build(), builder_b.build())),
            },
            NoiseBuilder::Max(builder_a, builder_b) => match *builder_a {
                NoiseBuilder::Constant(v) => dynn(Max::new(Constant::new(v), builder_b.build())),
                _ => dynn(Max::new(builder_a.build(), builder_b.build())),
            },
            NoiseBuilder::PowI(i, builder) => dynn(PowINoise(builder.build(), i)),
            NoiseBuilder::Pow(builder_a, builder_b) => match *builder_a {
                NoiseBuilder::Constant(v) => dynn(Power::new(Constant::new(v), builder_b.build())),
                _ => dynn(Power::new(builder_a.build(), builder_b.build())),
            },
            NoiseBuilder::ScaleInput(x, y, builder) => dynn(
                ScalePoint::new(builder.build())
                    .set_x_scale(x)
                    .set_y_scale(y),
            ),
            NoiseBuilder::Clamp(min, max, builder) => {
                dynn(Clamp::new(builder.build()).set_bounds(min, max))
            }
            NoiseBuilder::Checkerboard => dynn(Checkerboard::new(0)),
            NoiseBuilder::Perlin(seed) => dynn(Perlin::new(seed)),
            NoiseBuilder::Simplex(seed) => dynn(Simplex::new(seed)),
        }
    }

    /// Parses a simple format for defining noise.  
    /// Splits input into tokens by whitespace, and expects a single expression as input.
    /// There are no parenthsies, so an expression could be something like `add {expr} {expr}`
    /// Tokens are lowercase and named the same as their [NoiseBuilder] counterparts, except [Constant](NoiseBuilder::Constant) is just `c` and [ScaleInput](NoiseBuilder::ScaleInput) is `scalein`.  
    /// An example is given in 'terrain.yml', note that the formattig does not matter, as any whitspace causes a new token.  
    /// When using an expression that takes 2 expressions with a constant, the constant should be supplied first
    pub fn parse(string: &str) -> Result<Self, String> {
        let mut tokens = string.split_whitespace();
        let res = Self::from_tokens(&mut tokens);
        if !tokens.next().is_none() {
            Err("too many tokens".into())
        } else {
            res
        }
    }

    fn from_tokens(tokens: &mut SplitWhitespace) -> Result<Self, String> {
        let next = tokens.next();
        match next {
            Some(t) => match t {
                "c" => Ok(Self::Constant(parse(tokens)?)),
                "abs" => Ok(Self::Abs(eval(tokens)?)),
                "neg" => Ok(Self::Neg(eval(tokens)?)),
                "add" => Ok(Self::Add(eval(tokens)?, eval(tokens)?)),
                "mul" => Ok(Self::Mul(eval(tokens)?, eval(tokens)?)),
                "min" => Ok(Self::Min(eval(tokens)?, eval(tokens)?)),
                "max" => Ok(Self::Max(eval(tokens)?, eval(tokens)?)),
                "pow" => Ok(Self::Pow(eval(tokens)?, eval(tokens)?)),
                "powi" => Ok(Self::PowI(parse(tokens)?, eval(tokens)?)),
                "scalein" => Ok(Self::ScaleInput(
                    parse(tokens)?,
                    parse(tokens)?,
                    eval(tokens)?,
                )),
                "clamp" => Ok(Self::Clamp(parse(tokens)?, parse(tokens)?, eval(tokens)?)),
                "checkerboard" => Ok(Self::Checkerboard),
                "perlin" => Ok(Self::Perlin(parse(tokens)?)),
                "simplex" => Ok(Self::Simplex(parse(tokens)?)),
                _ => Err(format!("Invalid token: '{t}'")),
            },
            None => Err("Not enough tokens".into()),
        }
    }
}

fn eval(tokens: &mut SplitWhitespace) -> Result<Box<NoiseBuilder>, String> {
    match NoiseBuilder::from_tokens(tokens) {
        Ok(v) => Ok(Box::new(v)),
        Err(e) => Err(e),
    }
}

fn parse<T: FromStr>(tokens: &mut SplitWhitespace) -> Result<T, String> {
    match tokens.next() {
        Some(v) => match v.parse() {
            Ok(v) => Ok(v),
            Err(_) => Err(format!("could not parse float '{v}'")),
        },
        None => Err("Expected float, but ran out of tokens".into()),
    }
}

pub struct DynNoise(Box<dyn NoiseFn<f64, 2> + Send + Sync>);

impl NoiseFn<f64, 2> for DynNoise {
    #[inline]
    fn get(&self, point: [f64; 2]) -> f64 {
        self.0.get(point)
    }
}

fn dynn(source: impl NoiseFn<f64, 2> + 'static + Send + Sync) -> DynNoise {
    DynNoise(Box::new(source))
}
struct PowINoise<T: NoiseFn<f64, 2>>(T, i32);

impl<T: NoiseFn<f64, 2>> NoiseFn<f64, 2> for PowINoise<T> {
    #[inline]
    fn get(&self, point: [f64; 2]) -> f64 {
        self.0.get(point).powi(self.1)
    }
}
