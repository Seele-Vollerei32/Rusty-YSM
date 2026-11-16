//! Bounded arithmetic Molang expressions. Unsupported syntax is an error.
#[derive(Clone, Debug)]
pub enum Expr {
    Number(f32),
    Query(String),
    Unary(u8, Box<Expr>),
    Binary(String, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
    Select(Box<Expr>, Box<Expr>, Box<Expr>),
}
impl Expr {
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.len() > 4096 {
            return Err("expression exceeds 4096 bytes".into());
        }
        let text = text
            .trim()
            .strip_prefix("return ")
            .unwrap_or(text.trim())
            .trim_end_matches(';');
        let mut p = Parser {
            s: text.as_bytes(),
            i: 0,
            depth: 0,
        };
        let e = p.expression(0)?;
        p.space();
        if p.i != p.s.len() {
            return Err(format!("unsupported Molang at byte {}", p.i));
        }
        Ok(e)
    }
    pub fn eval(&self, query: &impl Fn(&str) -> f32) -> f32 {
        let value = match self {
            Self::Number(n) => *n,
            Self::Query(q) => query(q),
            Self::Unary(op, e) => {
                let v = e.eval(query);
                if *op == b'-' {
                    -v
                } else if *op == b'!' {
                    f32::from(v == 0.)
                } else {
                    v
                }
            }
            Self::Select(c, a, b) => {
                if c.eval(query) != 0. {
                    a.eval(query)
                } else {
                    b.eval(query)
                }
            }
            Self::Binary(op, a, b) => {
                let a = a.eval(query);
                if op == "&&" && a == 0. {
                    return 0.;
                }
                if op == "||" && a != 0. {
                    return 1.;
                }
                let b = b.eval(query);
                match op.as_str() {
                    "+" => a + b,
                    "-" => a - b,
                    "*" => a * b,
                    "/" if b != 0. => a / b,
                    "%" if b != 0. => a % b,
                    "/" | "%" => 0.,
                    "<" => f32::from(a < b),
                    ">" => f32::from(a > b),
                    "<=" => f32::from(a <= b),
                    ">=" => f32::from(a >= b),
                    "==" => f32::from(a == b),
                    "!=" => f32::from(a != b),
                    "&&" => f32::from(b != 0.),
                    "||" => f32::from(b != 0.),
                    _ => 0.,
                }
            }
            Self::Call(name, args) => {
                let v: Vec<f32> = args.iter().map(|a| a.eval(query)).collect();
                match name.as_str() {
                    "math.sin" => v[0].to_radians().sin(),
                    "math.cos" => v[0].to_radians().cos(),
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
                    _ => 0.,
                }
            }
        };
        if value.is_finite() { value } else { 0. }
    }
}
struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    depth: usize,
}
impl Parser<'_> {
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
    fn expression(&mut self, min: u8) -> Result<Expr, String> {
        self.depth += 1;
        if self.depth > 64 {
            return Err("expression nesting exceeds 64".into());
        }
        self.space();
        let mut left = if self.eat("-") {
            Expr::Unary(b'-', Box::new(self.expression(7)?))
        } else if self.eat("!") {
            Expr::Unary(b'!', Box::new(self.expression(7)?))
        } else if self.eat("+") {
            self.expression(7)?
        } else if self.eat("(") {
            let e = self.expression(0)?;
            if !self.eat(")") {
                return Err("missing )".into());
            }
            e
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
                    let n = match name.as_str() {
                        "math.sin" | "math.cos" | "math.abs" | "math.sqrt" | "math.floor"
                        | "math.ceil" | "math.round" => 1,
                        "math.min" | "math.max" | "math.pow" => 2,
                        "math.clamp" | "math.lerp" => 3,
                        _ => return Err(format!("unsupported function {name}")),
                    };
                    if args.len() != n {
                        return Err(format!("wrong arity for {name}"));
                    }
                    Expr::Call(name, args)
                } else if name == "math.pi" {
                    Expr::Number(std::f32::consts::PI)
                } else if name == "true" {
                    Expr::Number(1.)
                } else if name == "false" {
                    Expr::Number(0.)
                } else if let Some(q) = name
                    .strip_prefix("query.")
                    .or_else(|| name.strip_prefix("q."))
                {
                    Expr::Query(q.into())
                } else {
                    return Err(format!("unsupported identifier {name}"));
                }
            }
        };
        loop {
            self.space();
            let mut found = None;
            for (op, p) in [
                ("||", 1),
                ("&&", 2),
                ("==", 3),
                ("!=", 3),
                ("<=", 4),
                (">=", 4),
                ("<", 4),
                (">", 4),
                ("+", 5),
                ("-", 5),
                ("*", 6),
                ("/", 6),
                ("%", 6),
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
        if min == 0 && self.eat("?") {
            let yes = self.expression(0)?;
            if !self.eat(":") {
                return Err("missing ternary colon".into());
            }
            let no = self.expression(0)?;
            left = Expr::Select(Box::new(left), Box::new(yes), Box::new(no));
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
    fn conditional_and_validation() {
        assert_eq!(
            Expr::parse("q.is_moving ? math.clamp(5,0,2) : 9")
                .unwrap()
                .eval(&|_| 1.),
            2.
        );
        assert!(Expr::parse("math.sin()").is_err());
        assert!(Expr::parse("variable.x=1").is_err());
    }
}
