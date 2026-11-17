//! Bounded arithmetic Molang expressions. Unsupported syntax is an error.
#[derive(Clone, Debug)]
pub enum Expr {
    Number(f32),
    Query(String),
    Unary(u8, Box<Expr>),
    Binary(String, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
    Select(Box<Expr>, Box<Expr>, Box<Expr>),
    /// `variable.x = value`: stores the value and yields it.
    Assign(String, Box<Expr>),
    /// `a; b; return c`: each entry is (is a `return`, statement).
    Sequence(Vec<(bool, Expr)>),
}
/// Values assigned by expressions, keyed by `variable.<name>`.
pub type Variables = std::cell::RefCell<std::collections::HashMap<String, f32>>;
const MAX_VARIABLES: usize = 256;
const MAX_STATEMENTS: usize = 64;
impl Expr {
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.len() > 4096 {
            return Err("expression exceeds 4096 bytes".into());
        }
        let text = text.trim_end_matches(|c: char| c == ';' || c.is_whitespace());
        let mut p = Parser {
            s: text.as_bytes(),
            i: 0,
            depth: 0,
        };
        let mut statements = Vec::new();
        loop {
            p.space();
            let rest = &p.s[p.i..];
            let returns = rest.starts_with(b"return")
                && rest.get(6).is_some_and(|c| c.is_ascii_whitespace() || *c == b'(');
            if returns {
                p.i += 6;
            }
            statements.push((returns, p.expression(0)?));
            p.space();
            if p.i == p.s.len() {
                break;
            }
            if p.s[p.i] != b';' {
                return Err(format!("unsupported Molang at byte {}", p.i));
            }
            p.i += 1;
            if statements.len() >= MAX_STATEMENTS {
                return Err("too many statements".into());
            }
        }
        Ok(if statements.len() == 1 {
            statements.pop().unwrap().1
        } else {
            Self::Sequence(statements)
        })
    }
    /// Evaluates with a throwaway variable scope: assignments last for this call only.
    pub fn eval(&self, query: &impl Fn(&str) -> f32) -> f32 {
        self.eval_with(query, &Variables::default())
    }
    /// `variables` keeps what expressions assigned. A stored variable takes
    /// precedence over `query` for the same `variable.*` name.
    pub fn eval_with(&self, query: &impl Fn(&str) -> f32, variables: &Variables) -> f32 {
        let value = match self {
            Self::Number(n) => *n,
            Self::Query(q) => {
                let stored = variables.borrow().get(q).copied();
                stored.unwrap_or_else(|| query(q))
            }
            Self::Assign(name, e) => {
                let v = e.eval_with(query, variables);
                let mut store = variables.borrow_mut();
                // Bounded: past the cap a new name is not stored and reads as before.
                if store.len() < MAX_VARIABLES || store.contains_key(name) {
                    store.insert(name.clone(), v);
                }
                v
            }
            Self::Sequence(statements) => {
                let mut last = 0.;
                for (returns, e) in statements {
                    last = e.eval_with(query, variables);
                    if *returns {
                        break;
                    }
                }
                last
            }
            Self::Unary(op, e) => {
                let v = e.eval_with(query, variables);
                if *op == b'-' {
                    -v
                } else if *op == b'!' {
                    f32::from(v == 0.)
                } else {
                    v
                }
            }
            Self::Select(c, a, b) => {
                if c.eval_with(query, variables) != 0. {
                    a.eval_with(query, variables)
                } else {
                    b.eval_with(query, variables)
                }
            }
            Self::Binary(op, a, b) => {
                let a = a.eval_with(query, variables);
                if op == "&&" && a == 0. {
                    return 0.;
                }
                if op == "||" && a != 0. {
                    return 1.;
                }
                let b = b.eval_with(query, variables);
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
                let v: Vec<f32> = args.iter().map(|a| a.eval_with(query, variables)).collect();
                match name.as_str() {
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
                    "math.lerp_rotate" => v[0] + ((v[1] - v[0] + 180.).rem_euclid(360.) - 180.) * v[2],
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
                        | "math.ceil" | "math.round" | "math.asin" | "math.acos"
                        | "math.atan" | "math.trunc" | "math.exp" | "math.ln" => 1,
                        "math.min" | "math.max" | "math.pow" | "math.atan2" | "math.mod" => 2,
                        "math.clamp" | "math.lerp" | "math.lerp_rotate" => 3,
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
                } else if let Some(variable) = name.strip_prefix("variable.").or_else(|| name.strip_prefix("v.")) {
                    if variable.is_empty() {
                        return Err("empty variable name".into());
                    }
                    let key = format!("variable.{variable}");
                    self.space();
                    let rest = &self.s[self.i..];
                    if min == 0 && rest.first() == Some(&b'=') && rest.get(1) != Some(&b'=') {
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
        self.space();
        if self.s[self.i..].starts_with(b"??") {
            // An unset variable reads as 0 here, so `a ?? b` cannot be decided.
            return Err("null coalescing is not supported".into());
        }
        if min == 0 && self.eat("?") {
            let yes = self.expression(0)?;
            // OpenYSM's binary conditional `a ? b` yields nothing (0) when false.
            let no = if self.eat(":") {
                self.expression(0)?
            } else {
                Expr::Number(0.)
            };
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
        assert_eq!(scope.borrow().len(), MAX_VARIABLES);
        assert_eq!(Expr::parse("v.n0 = 7").unwrap().eval_with(&none, &scope), 7.);
        assert_eq!(scope.borrow()["variable.n0"], 7.);
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
        for invalid in ["ysm.", "ctrl.", "ysm.second_order(1,2)", "v.size ?? 1", "1 ?", "other.x"] {
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
}
