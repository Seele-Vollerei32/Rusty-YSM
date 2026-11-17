//! Bounded Molang expressions with OpenYSM's value model. Unsupported syntax is an error.
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::Arc;
/// OpenYSM's runtime values: null, booleans, floats and string literals.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f32),
    Str(Arc<str>),
}
impl Value {
    /// `ValueConversions.asFloat`: null and NaN are 0, a string is 1.
    pub fn as_f32(&self) -> f32 {
        match self {
            Self::Null => 0.,
            Self::Bool(b) => f32::from(*b),
            Self::Number(n) if n.is_nan() => 0.,
            Self::Number(n) => *n,
            Self::Str(_) => 1.,
        }
    }
    /// `ValueConversions.asBoolean`: null, 0 and NaN are false, a string is true.
    pub fn as_bool(&self) -> bool {
        match self {
            Self::Null => false,
            Self::Bool(b) => *b,
            Self::Number(n) => !n.is_nan() && *n != 0.,
            Self::Str(_) => true,
        }
    }
}
/// Every number an expression produces is finite; anything else is 0.
fn number(n: f32) -> Value {
    Value::Number(if n.is_finite() { n } else { 0. })
}
/// `==` as OpenYSM evaluates it: numeric as soon as either side is a number.
fn equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(_), _) | (_, Value::Number(_)) => a.as_f32() == b.as_f32(),
        (Value::Null, Value::Null) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        _ => false,
    }
}
/// `!=` is not the negation of `==` upstream: two booleans never differ.
fn differ(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(_), _) | (_, Value::Number(_)) => a.as_f32() != b.as_f32(),
        (Value::Null, Value::Null) => false,
        (Value::Null, _) | (_, Value::Null) => true,
        (Value::Str(x), Value::Str(y)) => x != y,
        (Value::Str(_), _) | (_, Value::Str(_)) => true,
        _ => false,
    }
}
/// The lower bound and width of a random range; reversed bounds are swapped.
fn span(min: f32, max: f32) -> (f32, f32) {
    if min > max { (max, min - max) } else { (min, max - min) }
}
fn span_i32(min: i32, max: i32) -> (i32, i32) {
    if min > max { (max, min.wrapping_sub(max)) } else { (min, max.wrapping_sub(min)) }
}
#[derive(Clone, Debug)]
pub enum Expr {
    Number(f32),
    /// A `'literal'`: names a physics value or compares with another string.
    Str(Arc<str>),
    Query(String),
    Unary(u8, Box<Expr>),
    Binary(String, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
    Select(Box<Expr>, Box<Expr>, Box<Expr>),
    /// `a ? b` without an else: null when `a` is false.
    Conditional(Box<Expr>, Box<Expr>),
    /// `a ?? b`: `b` only when `a` is null.
    Coalesce(Box<Expr>, Box<Expr>),
    /// `variable.x = value`: stores the value and yields it.
    Assign(String, Box<Expr>),
    /// `a; b; return c`: each entry is (is a `return`, statement).
    Sequence(Vec<(bool, Expr)>),
}
/// A named `ysm.first_order`/`ysm.second_order` value (OpenYSM's FirstOrder/SecondOrder).
#[derive(Debug)]
enum Physics {
    First { input: f32, response: f32, value: f32 },
    Second { input: f32, frequency: f32, damping: f32, response: f32, previous_input: f32, value: f32, velocity: f32 },
}
impl Physics {
    /// `setArgs`: the arguments of this frame, in the caller's order.
    fn set(&mut self, new_input: f32, a: f32, b: f32, c: f32) {
        match self {
            Self::First { input, response, .. } => {
                *input = new_input;
                *response = a;
            }
            Self::Second { input, frequency, damping, response, .. } => {
                *input = new_input;
                *frequency = a;
                *damping = b;
                *response = c;
            }
        }
    }
    fn value(&self) -> f32 {
        match self {
            Self::First { value, .. } | Self::Second { value, .. } => *value,
        }
    }
    fn update(&mut self, step: f32) {
        match self {
            Self::First { input, response, value } => {
                *value = (1. - step / *response) * *value + step / *response * *input;
            }
            Self::Second { input, frequency, damping, response, previous_input, value, velocity } => {
                use std::f32::consts::PI;
                let (input, frequency, damping) = (*input, frequency.clamp(0., 5.), damping.clamp(0., 1.));
                let k1 = damping / PI / frequency;
                let k2 = 1. / (2. * PI * frequency) / (2. * PI * frequency);
                let k3 = *response * damping / 2. / PI / frequency;
                let input_rate = (input - *previous_input) / step;
                *previous_input = input;
                // Substeps keep the integration stable; a zero frequency yields none.
                let max_step = (4. * k2 + k1 * k1).sqrt() - k1;
                let cycles = (step / max_step).ceil() as i32;
                if cycles > MAX_PHYSICS_STEPS {
                    // Upstream would integrate every substep. After a gap this long
                    // the value is taken as settled on its input instead.
                    *value = input;
                    *velocity = 0.;
                    return;
                }
                let step = step / cycles as f32;
                for _ in 0..cycles {
                    *value += step * *velocity;
                    *velocity += step * (k3 * input_rate + input - *value - k1 * *velocity) / k2;
                }
            }
        }
    }
}
/// One entity's Molang state: the variables its expressions assigned, its
/// named physics values and its random stream.
#[derive(Debug)]
pub struct Variables {
    values: RefCell<HashMap<String, Value>>,
    physics: RefCell<HashMap<Arc<str>, Physics>>,
    last_ticks: Cell<f32>,
    random: Cell<u64>,
}
impl Default for Variables {
    fn default() -> Self {
        // Each scope draws its own sequence.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0x9e37_79b9_7f4a_7c15);
        Self::with_seed(NEXT.fetch_add(0x9e37_79b9_7f4a_7c15, std::sync::atomic::Ordering::Relaxed))
    }
}
impl Variables {
    /// A scope whose `math.random` family is reproducible.
    pub fn with_seed(seed: u64) -> Self {
        Self {
            values: RefCell::default(),
            physics: RefCell::default(),
            last_ticks: Cell::new(0.),
            random: Cell::new(seed | 1),
        }
    }
    /// Forgets variables and physics, as when the entity's animation restarts.
    pub fn clear(&self) {
        self.values.borrow_mut().clear();
        self.physics.borrow_mut().clear();
        self.last_ticks.set(0.);
    }
    pub fn get(&self, name: &str) -> Option<Value> {
        self.values.borrow().get(name).cloned()
    }
    pub fn number(&self, name: &str) -> Option<f32> {
        self.get(name).map(|v| v.as_f32())
    }
    pub fn len(&self) -> usize {
        self.values.borrow().len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// `PhysicsManager.update`: call once before each evaluation with the
    /// entity's render time in ticks. Physics values integrate over the time
    /// since the previous call; a repeated or earlier time changes nothing.
    pub fn advance(&self, render_ticks: f32) {
        let last = self.last_ticks.get();
        if last > 0. {
            if render_ticks > last {
                let interval = (render_ticks - last) / 20.;
                self.last_ticks.set(render_ticks);
                for physics in self.physics.borrow_mut().values_mut() {
                    physics.update(interval);
                }
            }
        } else {
            self.last_ticks.set(render_ticks);
        }
    }
    fn next_u32(&self) -> u32 {
        // xorshift64*; the state is never 0.
        let mut x = self.random.get();
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.random.set(x);
        (x.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 32) as u32
    }
    /// In `[0, 1)`, like `RandomSource.nextFloat`.
    fn next_f32(&self) -> f32 {
        (self.next_u32() >> 8) as f32 / 16_777_216.
    }
    /// In `[0, bound)`. Upstream throws for an empty range; here it is 0.
    fn next_below(&self, bound: i32) -> i32 {
        if bound <= 0 { 0 } else { ((u64::from(self.next_u32()) * bound as u64) >> 32) as i32 }
    }
}
/// Why an expression was not compiled.
#[derive(Debug)]
pub struct ParseError {
    pub message: String,
    /// The text is not Molang for OpenYSM either, which evaluates it as 0.
    pub malformed: bool,
}
const MAX_VARIABLES: usize = 256;
const MAX_PHYSICS: usize = 256;
const MAX_PHYSICS_STEPS: i32 = 4096;
/// Upstream loops as often as a die roll asks for.
const MAX_ROLLS: i32 = 1024;
const MAX_STATEMENTS: usize = 64;
impl Expr {
    pub fn parse(text: &str) -> Result<Self, String> {
        Self::parse_checked(text).map_err(|error| error.message)
    }
    /// Like `parse`, and tells text OpenYSM cannot parse either from Molang this engine lacks.
    pub fn parse_checked(text: &str) -> Result<Self, ParseError> {
        if text.len() > 4096 {
            return Err(ParseError { message: "expression exceeds 4096 bytes".into(), malformed: false });
        }
        let text = text.trim_end_matches(|c: char| c == ';' || c.is_whitespace());
        let mut p = Parser {
            s: text.as_bytes(),
            i: 0,
            depth: 0,
            assign: 0,
        };
        p.statements().map_err(|message| {
            p.space();
            let rest = &p.s[p.i.min(p.s.len())..];
            // Scopes, indexing, arrows and assignment to a non-variable are Molang
            // OpenYSM accepts; so may be an identifier outside ASCII.
            let known = ["{", "}", "[", "]", "="].iter().any(|token| rest.starts_with(token.as_bytes()))
                || rest.first().is_some_and(|c| !c.is_ascii())
                // The arrow is read as a minus first, so the failure is one byte late.
                || text.contains("->");
            let malformed = !known
                && ["missing )", "expected comma", "expected expression", "unsupported Molang at byte", "unterminated string"]
                    .iter()
                    .any(|prefix| message.starts_with(prefix));
            ParseError { message, malformed }
        })
    }
    /// Evaluates with a throwaway variable scope: assignments last for this call only.
    pub fn eval(&self, query: &impl Fn(&str) -> f32) -> f32 {
        self.eval_with(query, &Variables::default())
    }
    /// `variables` keeps what expressions assigned. A stored variable takes
    /// precedence over `query` for the same `variable.*` name.
    pub fn eval_with(&self, query: &impl Fn(&str) -> f32, variables: &Variables) -> f32 {
        self.eval_value(query, variables).as_f32()
    }
    /// The typed result. A `query` answer of NaN means the name has no value:
    /// it reads as null, which `??` replaces and arithmetic takes as 0.
    pub fn eval_value(&self, query: &impl Fn(&str) -> f32, variables: &Variables) -> Value {
        match self {
            Self::Number(n) => number(*n),
            Self::Str(s) => Value::Str(s.clone()),
            Self::Query(q) => {
                let stored = variables.values.borrow().get(q).cloned();
                stored.unwrap_or_else(|| {
                    let v = query(q);
                    if v.is_nan() { Value::Null } else { number(v) }
                })
            }
            Self::Assign(name, e) => {
                let v = e.eval_value(query, variables);
                let mut store = variables.values.borrow_mut();
                // Bounded: past the cap a new name is not stored and reads as before.
                if store.len() < MAX_VARIABLES || store.contains_key(name) {
                    store.insert(name.clone(), v.clone());
                }
                v
            }
            Self::Sequence(statements) => {
                let mut last = Value::Number(0.);
                for (returns, e) in statements {
                    last = e.eval_value(query, variables);
                    if *returns {
                        break;
                    }
                }
                last
            }
            Self::Unary(op, e) => {
                let v = e.eval_value(query, variables);
                match op {
                    b'-' => number(-v.as_f32()),
                    b'!' => Value::Bool(!v.as_bool()),
                    _ => v,
                }
            }
            Self::Select(c, a, b) => {
                if c.eval_value(query, variables).as_bool() {
                    a.eval_value(query, variables)
                } else {
                    b.eval_value(query, variables)
                }
            }
            Self::Conditional(c, a) => {
                if c.eval_value(query, variables).as_bool() {
                    a.eval_value(query, variables)
                } else {
                    Value::Null
                }
            }
            Self::Coalesce(a, b) => match a.eval_value(query, variables) {
                Value::Null => b.eval_value(query, variables),
                v => v,
            },
            Self::Binary(op, a, b) => {
                let a = a.eval_value(query, variables);
                match op.as_str() {
                    "&&" => return Value::Bool(a.as_bool() && b.eval_value(query, variables).as_bool()),
                    "||" => return Value::Bool(a.as_bool() || b.eval_value(query, variables).as_bool()),
                    _ => {}
                }
                let b = b.eval_value(query, variables);
                match op.as_str() {
                    "==" => return Value::Bool(equal(&a, &b)),
                    "!=" => return Value::Bool(differ(&a, &b)),
                    _ => {}
                }
                let (a, b) = (a.as_f32(), b.as_f32());
                match op.as_str() {
                    "+" => number(a + b),
                    "-" => number(a - b),
                    "*" => number(a * b),
                    "/" if b != 0. => number(a / b),
                    "%" if b != 0. => number(a % b),
                    "<" => Value::Bool(a < b),
                    ">" => Value::Bool(a > b),
                    "<=" => Value::Bool(a <= b),
                    ">=" => Value::Bool(a >= b),
                    _ => Value::Number(0.),
                }
            }
            Self::Call(name, args) => {
                let arg = |i: usize| args[i].eval_value(query, variables).as_f32();
                match name.as_str() {
                    "math.random" => {
                        // A third argument is accepted and never evaluated.
                        let (min, range) = span(arg(0), arg(1));
                        number(min + variables.next_f32() * range)
                    }
                    "math.random_integer" | "math.randomi" => {
                        let (min, range) = span_i32(arg(0) as i32, arg(1) as i32);
                        Value::Number(min.wrapping_add(variables.next_below(range)) as f32)
                    }
                    "math.die_roll" | "math.roll" => {
                        let rolls = (arg(0) as i32).clamp(0, MAX_ROLLS);
                        let (min, range) = span(arg(1), arg(2));
                        let mut total = 0f32;
                        for _ in 0..rolls {
                            total += min + variables.next_f32() * range;
                        }
                        number(total)
                    }
                    "math.die_roll_integer" | "math.rolli" => {
                        let rolls = ((arg(0) + 0.5).floor() as i32).clamp(0, MAX_ROLLS);
                        let (min, range) = span_i32(arg(1) as i32, arg(2) as i32);
                        let mut total = 0i32;
                        for _ in 0..rolls {
                            total = total.wrapping_add(min.wrapping_add(variables.next_below(range)));
                        }
                        Value::Number(total as f32)
                    }
                    "ysm.second_order" | "ysm.first_order" => {
                        // The name must be a non-empty string; anything else is 0.
                        let Value::Str(key) = args[0].eval_value(query, variables) else {
                            return Value::Number(0.);
                        };
                        if key.is_empty() {
                            return Value::Number(0.);
                        }
                        let input = arg(1);
                        let second = name == "ysm.second_order";
                        // frequency, damping, response; first_order takes only a response.
                        let mut extra = [1f32; 3];
                        for (i, value) in extra.iter_mut().enumerate().take(if second { 3 } else { 1 }) {
                            if i + 2 < args.len() {
                                *value = arg(i + 2);
                            }
                        }
                        let mut physics = variables.physics.borrow_mut();
                        if let Some(existing) = physics.get_mut(&key) {
                            // The value was integrated by `Variables::advance`; this
                            // call only supplies the inputs of the next step.
                            if second {
                                existing.set(input, extra[0], extra[1], extra[2]);
                            } else {
                                existing.set(input, extra[0], 0., 0.);
                            }
                            return number(existing.value());
                        }
                        // Bounded: past the cap a new name is not tracked and follows its input.
                        if physics.len() < MAX_PHYSICS {
                            physics.insert(key, if second {
                                Physics::Second {
                                    input,
                                    frequency: extra[0].clamp(0., 5.),
                                    damping: extra[1].clamp(0., 1.),
                                    response: extra[2],
                                    previous_input: 0.,
                                    value: 0.,
                                    velocity: 0.,
                                }
                            } else {
                                Physics::First { input, response: extra[0], value: 0. }
                            });
                        }
                        number(input)
                    }
                    _ => {
                        let v: Vec<f32> = args.iter().map(|a| a.eval_value(query, variables).as_f32()).collect();
                        number(match name.as_str() {
                            "math.sin" => v[0].to_radians().sin(),
                            "math.cos" => v[0].to_radians().cos(),
                            "math.asin" => v[0].asin().to_degrees(),
                            "math.acos" => v[0].acos().to_degrees(),
                            "math.atan" => v[0].atan().to_degrees(),
                            "math.atan2" => v[0].atan2(v[1]).to_degrees(),
                            "math.trunc" => v[0].trunc(),
                            "math.exp" => v[0].exp(),
                            "math.ln" => v[0].ln(),
                            "math.mod" if v[1] != 0. => v[0] % v[1],
                            "math.mod" => 0.,
                            "math.lerp_rotate" | "math.lerprotate" => {
                                v[0] + ((v[1] - v[0] + 180.).rem_euclid(360.) - 180.) * v[2]
                            }
                            "math.abs" => v[0].abs(),
                            "math.sqrt" => v[0].max(0.).sqrt(),
                            "math.floor" => v[0].floor(),
                            "math.ceil" => v[0].ceil(),
                            "math.round" => v[0].round(),
                            "math.min" => v[0].min(v[1]),
                            "math.max" => v[0].max(v[1]),
                            "math.pow" => v[0].powf(v[1]),
                            "math.clamp" => v[0].max(v[1]).min(v[2]),
                            "math.lerp" => v[0] + (v[1] - v[0]) * v[2],
                            // Upstream rounds the input up and the result down.
                            "math.hermite_blend" | "math.hermite" => {
                                let t = f64::from(v[0].ceil() as i32);
                                (3. * t * t - 2. * t * t * t).floor() as i32 as f32
                            }
                            "math.min_angle" => {
                                let angle = v[0] % 360.;
                                if angle >= 180. {
                                    angle - 360.
                                } else if angle < -180. {
                                    angle + 360.
                                } else {
                                    angle
                                }
                            }
                            _ => 0.,
                        })
                    }
                }
            }
        }
    }
}
struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    depth: usize,
    /// The loosest level at which `v.x = value` may start; see `branch`.
    assign: u8,
}
impl Parser<'_> {
    fn statements(&mut self) -> Result<Expr, String> {
        let mut statements = Vec::new();
        loop {
            self.space();
            let rest = &self.s[self.i..];
            let returns = rest.starts_with(b"return")
                && rest.get(6).is_some_and(|c| c.is_ascii_whitespace() || *c == b'(');
            if returns {
                self.i += 6;
            }
            statements.push((returns, self.expression(0)?));
            self.space();
            if self.i == self.s.len() {
                break;
            }
            if self.s[self.i] != b';' {
                return Err(format!("unsupported Molang at byte {}", self.i));
            }
            self.i += 1;
            if statements.len() >= MAX_STATEMENTS {
                return Err("too many statements".into());
            }
        }
        Ok(if statements.len() == 1 {
            statements.pop().unwrap().1
        } else {
            Expr::Sequence(statements)
        })
    }
    fn space(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn eat(&mut self, t: &str) -> bool {
        self.space();
        if self.s[self.i..].starts_with(t.as_bytes()) {
            self.i += t.len();
            true
        } else {
            false
        }
    }
    /// One side of `a ? b : c`. Upstream parses it above `??`; an assignment
    /// written there without parentheses stays accepted.
    fn branch(&mut self) -> Result<Expr, String> {
        let outer = std::mem::replace(&mut self.assign, 2);
        let branch = self.expression(2);
        self.assign = outer;
        branch
    }
    /// Levels: 0 statement, 1 `??`, 2 `?`, 3 `||`, 4 `&&`, 5 equality, 6 comparison,
    /// 7 additive, 8 multiplicative, 9 unary operand.
    fn expression(&mut self, min: u8) -> Result<Expr, String> {
        self.depth += 1;
        if self.depth > 64 {
            return Err("expression nesting exceeds 64".into());
        }
        self.space();
        let mut left = if self.eat("-") {
            Expr::Unary(b'-', Box::new(self.expression(9)?))
        } else if self.eat("!") {
            Expr::Unary(b'!', Box::new(self.expression(9)?))
        } else if self.eat("+") {
            self.expression(9)?
        } else if self.eat("(") {
            let e = self.expression(0)?;
            if !self.eat(")") {
                return Err("missing )".into());
            }
            e
        } else if self.eat("'") {
            // No escapes, as upstream: the literal ends at the next quote.
            let start = self.i;
            let Some(length) = self.s[start..].iter().position(|c| *c == b'\'') else {
                return Err("unterminated string".into());
            };
            self.i = start + length + 1;
            Expr::Str(std::str::from_utf8(&self.s[start..start + length]).map_err(|_| "invalid string")?.into())
        } else {
            self.space();
            let start = self.i;
            if self
                .s
                .get(self.i)
                .is_some_and(|c| c.is_ascii_digit() || *c == b'.')
            {
                while self
                    .s
                    .get(self.i)
                    .is_some_and(|c| c.is_ascii_digit() || *c == b'.')
                {
                    self.i += 1;
                }
                if self.s.get(self.i).is_some_and(|c| *c == b'e' || *c == b'E') {
                    self.i += 1;
                    if self.s.get(self.i).is_some_and(|c| *c == b'+' || *c == b'-') {
                        self.i += 1;
                    }
                    while self.s.get(self.i).is_some_and(u8::is_ascii_digit) {
                        self.i += 1;
                    }
                }
                let n = std::str::from_utf8(&self.s[start..self.i])
                    .unwrap()
                    .parse::<f32>()
                    .map_err(|_| "invalid number")?;
                if !n.is_finite() {
                    return Err("nonfinite number".into());
                }
                Expr::Number(n)
            } else {
                while self
                    .s
                    .get(self.i)
                    .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'.')
                {
                    self.i += 1;
                }
                if self.i == start {
                    return Err("expected expression".into());
                }
                let name = std::str::from_utf8(&self.s[start..self.i])
                    .unwrap()
                    .to_ascii_lowercase();
                if self.eat("(") {
                    let mut args = Vec::new();
                    if !self.eat(")") {
                        loop {
                            args.push(self.expression(0)?);
                            if self.eat(")") {
                                break;
                            }
                            if !self.eat(",") {
                                return Err("expected comma".into());
                            }
                        }
                    }
                    let valid = match name.as_str() {
                        "math.sin" | "math.cos" | "math.abs" | "math.sqrt" | "math.floor"
                        | "math.ceil" | "math.round" | "math.asin" | "math.acos"
                        | "math.atan" | "math.trunc" | "math.exp" | "math.ln"
                        | "math.hermite_blend" | "math.hermite" | "math.min_angle" => args.len() == 1,
                        "math.min" | "math.max" | "math.pow" | "math.atan2" | "math.mod"
                        | "math.random_integer" | "math.randomi" => args.len() == 2,
                        "math.clamp" | "math.lerp" | "math.lerp_rotate" | "math.lerprotate"
                        | "math.die_roll" | "math.roll" | "math.die_roll_integer" | "math.rolli" => args.len() == 3,
                        "math.random" => matches!(args.len(), 2 | 3),
                        "ysm.second_order" | "ysm.first_order" => args.len() >= 2,
                        _ => return Err(format!("unsupported function {name}")),
                    };
                    if !valid {
                        return Err(format!("wrong arity for {name}"));
                    }
                    Expr::Call(name, args)
                } else if name == "math.pi" {
                    Expr::Number(std::f32::consts::PI)
                } else if name == "math.e" {
                    Expr::Number(std::f32::consts::E)
                } else if name == "true" {
                    Expr::Number(1.)
                } else if name == "false" {
                    Expr::Number(0.)
                } else if let Some(q) = name
                    .strip_prefix("query.")
                    .or_else(|| name.strip_prefix("q."))
                {
                    Expr::Query(q.into())
                } else if let Some(variable) = name.strip_prefix("variable.").or_else(|| name.strip_prefix("v.")) {
                    if variable.is_empty() {
                        return Err("empty variable name".into());
                    }
                    let key = format!("variable.{variable}");
                    self.space();
                    let rest = &self.s[self.i..];
                    if min <= self.assign && rest.first() == Some(&b'=') && rest.get(1) != Some(&b'=') {
                        // `v.x = value` yields the value, as in OpenYSM. Only where a
                        // whole expression may start, so `2 * v.x = 3` stays an error.
                        self.i += 1;
                        Expr::Assign(key, Box::new(self.expression(0)?))
                    } else {
                        Expr::Query(key)
                    }
                } else if (name.starts_with("ysm.") || name.starts_with("ctrl."))
                    && !name.ends_with('.')
                {
                    // OpenYSM bindings keep their namespace, so they cannot be
                    // mistaken for a `query.*` name. Unwired ones read as 0.
                    Expr::Query(name)
                } else {
                    return Err(format!("unsupported identifier {name}"));
                }
            }
        };
        loop {
            self.space();
            let mut found = None;
            for (op, p) in [
                ("||", 3),
                ("&&", 4),
                ("==", 5),
                ("!=", 5),
                ("<=", 6),
                (">=", 6),
                ("<", 6),
                (">", 6),
                ("+", 7),
                ("-", 7),
                ("*", 8),
                ("/", 8),
                ("%", 8),
            ] {
                if p >= min && self.s[self.i..].starts_with(op.as_bytes()) {
                    found = Some((op, p));
                    break;
                }
            }
            let Some((op, p)) = found else {
                break;
            };
            self.i += op.len();
            let right = self.expression(p + 1)?;
            left = Expr::Binary(op.into(), Box::new(left), Box::new(right));
        }
        self.space();
        let rest = &self.s[self.i..];
        if min <= 2 && rest.starts_with(b"?") && !rest.starts_with(b"??") {
            self.i += 1;
            let yes = self.branch()?;
            left = if self.eat(":") {
                Expr::Select(Box::new(left), Box::new(yes), Box::new(self.branch()?))
            } else {
                // OpenYSM's binary conditional `a ? b` yields null when false.
                Expr::Conditional(Box::new(left), Box::new(yes))
            };
        }
        while min <= 1 && self.eat("??") {
            let right = self.expression(2)?;
            left = Expr::Coalesce(Box::new(left), Box::new(right));
        }
        self.depth -= 1;
        Ok(left)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn precedence_and_degree_math() {
        assert_eq!(Expr::parse("1+2*3").unwrap().eval(&|_| 0.), 7.);
        assert!(
            (Expr::parse("math.sin(query.anim_time * 90)")
                .unwrap()
                .eval(&|_| 1.)
                - 1.)
                .abs()
                < 0.0001
        );
    }
    #[test]
    fn angle_functions_and_numeric_boundaries() {
        for (source, expected) in [
            ("math.asin(1)", 90.), ("math.acos(-1)", 180.),
            ("math.atan(1)", 45.), ("math.atan2(1,0)", 90.),
            ("math.lerp_rotate(170,-170,0.5)", 180.),
            ("math.lerp_rotate(-170,170,0.5)", -180.),
            ("math.trunc(-1.9)", -1.), ("math.mod(-5,3)", -2.),
            ("math.mod(1,0)", 0.), ("math.ln(math.exp(1))", 1.),
            ("math.asin(2)", 0.), ("math.ln(-1)", 0.),
        ] {
            let actual = Expr::parse(source).unwrap().eval(&|_| 0.);
            assert!((actual - expected).abs() < 0.0001, "{source}: {actual}");
        }
        assert!(Expr::parse("math.atan2(1)").is_err());
        assert!(Expr::parse("math.lerp_rotate(1,2)").is_err());
    }
    #[test]
    fn variable_reads_preserve_namespace_and_alias() {
        let expr = Expr::parse("variable.pose + v.pose + query.pose").unwrap();
        assert_eq!(expr.eval(&|name| match name {
            "variable.pose" => 3., "pose" => 10., _ => 0.,
        }), 16.);
        assert!(Expr::parse("variable.").is_err());
    }
    #[test]
    fn assignment_yields_its_value_and_persists_in_the_scope() {
        let scope = Variables::default();
        let none = |_: &str| 0.;
        // The shape the built-in idle/walk clips use as a channel value.
        let bob = Expr::parse("v.bv=math.cos(2*query.anim_time*360/(24/24))*3").unwrap();
        assert!((bob.eval_with(&|name| if name == "anim_time" { 0.5 } else { 0. }, &scope) - 3.).abs() < 1e-4);
        let read = Expr::parse("variable.bv + 1").unwrap();
        assert!((read.eval_with(&none, &scope) - 4.).abs() < 1e-4);
        // A stored variable wins over the external value of the same name.
        assert!((read.eval_with(&|name| if name == "variable.bv" { 50. } else { 0. }, &scope) - 4.).abs() < 1e-4);
        // Without a shared scope the assignment does not leak.
        assert_eq!(read.eval(&|name| if name == "variable.bv" { 50. } else { 0. }), 51.);
        // Statements run in order; the last value or an explicit return is the result.
        let scope = Variables::default();
        assert_eq!(Expr::parse("v.a=2;v.b=v.a*3;").unwrap().eval_with(&none, &scope), 6.);
        assert_eq!(Expr::parse("v.a=1; return v.a+v.b; v.a=9").unwrap().eval_with(&none, &scope), 7.);
        assert_eq!(Expr::parse("v.a").unwrap().eval_with(&none, &scope), 1.);
        assert_eq!(Expr::parse("return 5").unwrap().eval(&none), 5.);
        assert_eq!(Expr::parse("q.x ? (v.c = 4) : 2").unwrap().eval_with(&|_| 1., &scope), 4.);
        assert_eq!(Expr::parse("v.d = v.e = 3").unwrap().eval_with(&none, &scope), 3.);
        assert_eq!(Expr::parse("v.d + v.e").unwrap().eval_with(&none, &scope), 6.);
        // Non-finite results are stored as 0, like every other value.
        assert_eq!(Expr::parse("v.f = math.ln(-1)").unwrap().eval_with(&none, &scope), 0.);
        for invalid in ["2 * v.x = 3", "q.x = 1", "ysm.head_yaw = 1", "v.x =", "v.a=1;;v.b=2", "1 2", "= 1"] {
            assert!(Expr::parse(invalid).is_err(), "{invalid}");
        }
        assert!(Expr::parse(&"v.a=1;".repeat(MAX_STATEMENTS + 1)).is_err());
        // The scope is bounded: names past the cap are not stored.
        let scope = Variables::default();
        for i in 0..MAX_VARIABLES + 5 {
            Expr::parse(&format!("v.n{i} = 1")).unwrap().eval_with(&none, &scope);
        }
        assert_eq!(scope.len(), MAX_VARIABLES);
        assert_eq!(Expr::parse("v.n0 = 7").unwrap().eval_with(&none, &scope), 7.);
        assert_eq!(scope.number("variable.n0"), Some(7.));
    }
    #[test]
    fn binary_conditional_and_openysm_namespaces() {
        // The exact shape the built-in arm animations use.
        let expr = Expr::parse("(v.qh==1?(15)) + (v.qh==2?(-40)) + (v.jump?(7));").unwrap();
        let eval = |qh: f32, jump: f32| expr.eval(&|name| match name {
            "variable.qh" => qh, "variable.jump" => jump, _ => 0.,
        });
        assert_eq!(eval(1., 0.), 15.);
        assert_eq!(eval(2., 1.), -33.);
        assert_eq!(eval(0., 0.), 0.);
        assert_eq!(Expr::parse("0 ? 5").unwrap().eval(&|_| 0.), 0.);
        assert_eq!(Expr::parse("1 ? 2 ? 3 : 4").unwrap().eval(&|_| 0.), 3.);
        let bound = Expr::parse("-71 - 0.8*ysm.head_pitch + ctrl.elytra_fly + query.head_pitch").unwrap();
        assert_eq!(bound.eval(&|name| match name {
            "ysm.head_pitch" => 10., "ctrl.elytra_fly" => 1., "head_pitch" => 100., _ => 0.,
        }), 22.);
        for invalid in ["ysm.", "ctrl.", "ysm.second_order(1)", "ysm.unknown(1,2)", "v.size ??", "1 ?", "other.x"] {
            assert!(Expr::parse(invalid).is_err(), "{invalid}");
        }
    }
    #[test]
    fn conditional_and_validation() {
        assert_eq!(
            Expr::parse("q.is_moving ? math.clamp(5,0,2) : 9")
                .unwrap()
                .eval(&|_| 1.),
            2.
        );
        assert!(Expr::parse("math.sin()").is_err());
        assert!(Expr::parse("variable.x==").is_err());
    }
    #[test]
    fn typed_values_follow_openysm_conversions() {
        let none = |_: &str| 0.;
        let absent = |_: &str| f32::NAN;
        let eval = |source: &str| Expr::parse(source).unwrap().eval(&none);
        // Only null is replaced; 0 is a value.
        assert_eq!(eval("0 ?? 7"), 0.);
        assert_eq!(Expr::parse("v.missing ?? 7").unwrap().eval(&absent), 7.);
        assert_eq!(Expr::parse("v.missing ?? 7").unwrap().eval(&none), 0.);
        assert_eq!(Expr::parse("v.missing + 2").unwrap().eval(&absent), 2.);
        assert_eq!(Expr::parse("v.a ?? v.b ?? 4").unwrap().eval(&absent), 4.);
        // A false binary conditional is null; the ternary's branches bind tighter than ??.
        assert_eq!(eval("(0 ? 5) ?? 9"), 9.);
        assert_eq!(eval("0 ? 2 ?? 3"), 3.);
        assert_eq!(eval("1 ? 2 ?? 3"), 2.);
        assert_eq!(eval("0 ?? 1 ? 5 : 6"), 0.);
        assert_eq!(Expr::parse("v.missing ?? 0 ? 5 : 6").unwrap().eval(&absent), 6.);
        assert_eq!(eval("(0 ? 1) == (0 ? 1)"), 1.);
        assert_eq!(eval("(0 ? 1) == 0"), 1.);
        // Strings: 1 as a number, true as a condition, compared by content.
        assert_eq!(eval("'a' == 'a'"), 1.);
        assert_eq!(eval("'a' == 'b'"), 0.);
        assert_eq!(eval("'a' != 'b'"), 1.);
        assert_eq!(eval("'a' + 1"), 2.);
        assert_eq!(eval("!'a'"), 0.);
        assert_eq!(eval("'' ? 3 : 4"), 3.);
        assert_eq!(eval("'a' == 1"), 1.);
        assert_eq!(eval("(1 < 2) == 'a'"), 0.);
        // Upstream's != never tells two booleans apart.
        assert_eq!(eval("(1 < 2) != (2 < 1)"), 0.);
        assert_eq!(eval("(1 < 2) == (2 < 1)"), 0.);
        assert_eq!(eval("(1 < 2) == 1"), 1.);
        let scope = Variables::default();
        assert_eq!(Expr::parse("v.name = 'left'; v.name == 'left'").unwrap().eval_with(&none, &scope), 1.);
        assert_eq!(scope.get("variable.name"), Some(Value::Str("left".into())));
        assert_eq!(Expr::parse("v.gone = (0 ? 1); v.gone ?? 8").unwrap().eval_with(&none, &scope), 8.);
        assert_eq!(Expr::parse("1 ? 2 : 3").unwrap().eval_value(&none, &scope), Value::Number(2.));
        assert_eq!(Expr::parse("2 > 1").unwrap().eval_value(&none, &scope), Value::Bool(true));
        assert_eq!(Expr::parse("0 ? 1").unwrap().eval_value(&none, &scope), Value::Null);
        assert_eq!(eval("math.e"), std::f32::consts::E);
        assert_eq!(eval("math.min_angle(190)"), -170.);
        assert_eq!(eval("math.min_angle(-190)"), 170.);
        assert_eq!(eval("math.min_angle(180)"), -180.);
        assert_eq!(eval("math.hermite_blend(0.2)"), 1.);
        assert_eq!(eval("math.lerprotate(170,-170,0.5)"), 180.);
        for invalid in ["'open", "v.x ?? ", "?? 1", "'a' 'b'", "math.min_angle()", "math.hermite(1,2)"] {
            assert!(Expr::parse(invalid).is_err(), "{invalid}");
        }
    }
    #[test]
    fn random_family_matches_upstream_ranges() {
        let none = |_: &str| 0.;
        let scope = Variables::with_seed(7);
        let run = |source: &str| Expr::parse(source).unwrap().eval_with(&none, &scope);
        let mut seen = [false; 3];
        for _ in 0..300 {
            let v = run("math.random(2, 5)");
            assert!((2. ..5.).contains(&v), "{v}");
            // Reversed bounds are swapped, as in the built-in spell clips.
            let v = run("math.random(0.1, 0)");
            assert!((0. ..0.1).contains(&v), "{v}");
            let i = run("math.random_integer(0, 3)");
            assert!(i == i.trunc() && (0. ..3.).contains(&i), "{i}");
            seen[i as usize] = true;
            let i = run("math.randomi(-2, -4)");
            assert!((-4. ..-2.).contains(&i), "{i}");
            let d = run("math.die_roll(3, 1, 2)");
            assert!((3. ..6.).contains(&d), "{d}");
            let d = run("math.die_roll_integer(2.5, 1, 3)");
            assert!(d == d.trunc() && (3. ..=6.).contains(&d), "{d}");
        }
        assert_eq!(seen, [true; 3]);
        assert_eq!(run("math.random(1, 1)"), 1.);
        // Upstream throws on an empty integer range; here it is the lower bound.
        assert_eq!(run("math.random_integer(3, 3)"), 3.);
        assert_eq!(run("math.die_roll(3, 1, 1)"), 3.);
        assert_eq!(run("math.roll(0, 1, 9)"), 0.);
        assert_eq!(run("math.rolli(2, 5, 5)"), 10.);
        assert_eq!(run("math.die_roll(-4, 1, 9)"), 0.);
        // The third argument of random is accepted and not evaluated.
        run("math.random(0, 1, v.touched = 5)");
        assert_eq!(scope.get("variable.touched"), None);
        // The same seed gives the same sequence; scopes draw independently.
        let (a, b) = (Variables::with_seed(42), Variables::with_seed(42));
        let roll = Expr::parse("math.random(0, 100)").unwrap();
        let first: Vec<f32> = (0..8).map(|_| roll.eval_with(&none, &a)).collect();
        let second: Vec<f32> = (0..8).map(|_| roll.eval_with(&none, &b)).collect();
        assert_eq!(first, second);
        assert!(first.windows(2).any(|w| w[0] != w[1]));
        for invalid in ["math.random(1)", "math.random(1,2,3,4)", "math.random_integer(1)", "math.die_roll(1,2)"] {
            assert!(Expr::parse(invalid).is_err(), "{invalid}");
        }
    }
    #[test]
    fn second_order_integrates_once_per_advanced_frame() {
        let none = |_: &str| 0.;
        let scope = Variables::default();
        // frequency 1, damping 1, response 0: k1 = 1/pi, k2 = 1/(2 pi)^2, k3 = 0.
        let spring = Expr::parse("ysm.second_order('spring', 10, 1, 1, 0)").unwrap();
        // The first call registers the value and answers with its input.
        assert_eq!(spring.eval_with(&none, &scope), 10.);
        // The simulation itself starts from 0, and the first advance only records the time.
        scope.advance(1.);
        assert_eq!(spring.eval_with(&none, &scope), 0.);
        // One tick is a 0.05 s step: the velocity moves first, the value follows a step later.
        scope.advance(2.);
        assert_eq!(spring.eval_with(&none, &scope), 0.);
        scope.advance(3.);
        let expected = 0.05 * 0.05 * 10. * (2. * std::f32::consts::PI).powi(2);
        assert!((spring.eval_with(&none, &scope) - expected).abs() < 1e-4);
        // A repeated or earlier time, and repeated reads, change nothing.
        scope.advance(3.);
        scope.advance(2.5);
        assert!((spring.eval_with(&none, &scope) - expected).abs() < 1e-4);
        assert!((spring.eval_with(&none, &scope) - expected).abs() < 1e-4);
        // Critically damped: it settles on the input without passing it.
        let mut previous = 0.;
        for tick in 4..400 {
            scope.advance(tick as f32);
            let value = spring.eval_with(&none, &scope);
            assert!(value >= previous - 1e-4 && value <= 10.001, "{tick}: {value}");
            previous = value;
        }
        assert!((previous - 10.).abs() < 0.01, "{previous}");
        // A long frame is split into stable substeps.
        let other = Variables::default();
        let stiff = Expr::parse("ysm.second_order('s', 4, 5, 0.3, 0)").unwrap();
        stiff.eval_with(&none, &other);
        other.advance(1.);
        other.advance(201.);
        let value = stiff.eval_with(&none, &other);
        assert!(value.is_finite() && (value - 4.).abs() < 1., "{value}");
        // After a gap beyond the substep budget the value is taken as settled.
        other.advance(1.0e7);
        assert_eq!(stiff.eval_with(&none, &other), 4.);
        // Defaults are frequency 1, damping 1, response 1; the state is per scope.
        let defaults = Expr::parse("ysm.second_order('d', 6)").unwrap();
        assert_eq!(defaults.eval_with(&none, &other), 6.);
        assert_eq!(defaults.eval_with(&none, &Variables::default()), 6.);
        // A zero frequency never moves and never produces a non-finite value.
        let frozen = Expr::parse("ysm.second_order('z', 5, 0)").unwrap();
        frozen.eval_with(&none, &other);
        other.advance(1.1e7);
        assert_eq!(frozen.eval_with(&none, &other), 0.);
        // The name must be a non-empty string.
        for source in ["ysm.second_order(1, 5)", "ysm.second_order('', 5)", "ysm.second_order(v.none, 5)", "ysm.first_order(2, 5)"] {
            assert_eq!(Expr::parse(source).unwrap().eval_with(&none, &other), 0., "{source}");
        }
        // A variable may carry the name; the input is read every frame.
        let named = Expr::parse("v.n = 'follow'; ysm.second_order(v.n, q.target, 2, 1, 0)").unwrap();
        let target = std::cell::Cell::new(1f32);
        let query = |name: &str| if name == "target" { target.get() } else { 0. };
        let fresh = Variables::default();
        assert_eq!(named.eval_with(&query, &fresh), 1.);
        target.set(-3.);
        for tick in 1..300 {
            fresh.advance(tick as f32);
            named.eval_with(&query, &fresh);
        }
        assert!((named.eval_with(&query, &fresh) + 3.).abs() < 0.01);
        // Clearing forgets the simulation: the next call answers with its input again.
        fresh.clear();
        assert_eq!(named.eval_with(&query, &fresh), -3.);
        assert!(fresh.get("variable.n").is_some());
    }
    #[test]
    fn first_order_and_bounds() {
        let none = |_: &str| 0.;
        let scope = Variables::default();
        let filter = Expr::parse("ysm.first_order('f', 8, 0.5)").unwrap();
        assert_eq!(filter.eval_with(&none, &scope), 8.);
        scope.advance(1.);
        scope.advance(2.);
        // (1 - dt/response) * 0 + dt/response * 8 with dt = 0.05.
        assert!((filter.eval_with(&none, &scope) - 0.8).abs() < 1e-5);
        scope.advance(3.);
        assert!((filter.eval_with(&none, &scope) - 1.52).abs() < 1e-5);
        // A zero response divides by zero upstream; the result reads as 0 here.
        let broken = Expr::parse("ysm.first_order('g', 8, 0)").unwrap();
        broken.eval_with(&none, &scope);
        scope.advance(4.);
        assert_eq!(broken.eval_with(&none, &scope), 0.);
        // Names past the cap are not tracked: they keep answering with their input.
        let crowded = Variables::default();
        for i in 0..MAX_PHYSICS {
            Expr::parse(&format!("ysm.first_order('p{i}', 1)")).unwrap().eval_with(&none, &crowded);
        }
        let extra = Expr::parse("ysm.second_order('extra', 9)").unwrap();
        crowded.advance(1.);
        crowded.advance(2.);
        assert_eq!(extra.eval_with(&none, &crowded), 9.);
        crowded.advance(3.);
        assert_eq!(extra.eval_with(&none, &crowded), 9.);
    }
    #[test]
    fn malformed_text_is_told_from_unsupported_molang() {
        // Shapes found in built-in packs: a stray parenthesis ends the expression early.
        for source in ["(-7.5+(1-0.7)*ysm.head_pitch))+(ctrl.elytra_fly==true?-30:0)", "math.sin(q.x))*2", "1 +", "(1", "math.min(1 2)", "'open", "1 2", "\"a\""] {
            let error = Expr::parse_checked(source).unwrap_err();
            assert!(error.malformed, "{source}: {}", error.message);
        }
        // Molang OpenYSM evaluates and this engine does not, and this engine's own limits.
        let long = "1+".repeat(2100) + "1";
        for source in ["ysm.bone_rot('Head')", "other.x", "{v.x=1;}", "v.list[0]", "2 * v.x = 3", "v.a -> v.b", "math.min(1)", long.as_str()] {
            let error = Expr::parse_checked(source).unwrap_err();
            assert!(!error.malformed, "{source}: {}", error.message);
        }
        assert_eq!(Expr::parse("1 +").unwrap_err(), Expr::parse_checked("1 +").unwrap_err().message);
    }
}
