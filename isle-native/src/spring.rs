//! The same spring the PowerShell build uses, so the feel is not re-invented:
//! four independent springs (width, height, radius, shoulder), stiffness 155,
//! damping 18 for the sizes and 25 for the shape, sub-stepped at 1/240 s.

#[derive(Clone, Copy)]
pub struct Spring {
    pub value: f32,
    pub target: f32,
    pub velocity: f32,
    stiffness: f32,
    damping: f32,
}

impl Spring {
    pub fn new(value: f32, stiffness: f32, damping: f32) -> Self {
        Self {
            value,
            target: value,
            velocity: 0.0,
            stiffness,
            damping,
        }
    }

    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    fn settled(&self) -> bool {
        (self.value - self.target).abs() < 0.25 && self.velocity.abs() < 2.0
    }

    /// Steps the spring by `dt` seconds and reports whether it has come to rest.
    fn step(&mut self, dt: f32) -> bool {
        const MAX_STEP: f32 = 1.0 / 240.0;
        let mut remaining = dt.clamp(0.0, 0.25);
        while remaining > 0.0 && !self.settled() {
            let h = remaining.min(MAX_STEP);
            let acceleration =
                -self.stiffness * (self.value - self.target) - self.damping * self.velocity;
            self.velocity += acceleration * h;
            self.value += self.velocity * h;
            remaining -= h;
        }
        if self.settled() {
            self.value = self.target;
            self.velocity = 0.0;
            return true;
        }
        false
    }
}

pub struct Animator {
    pub width: Spring,
    pub height: Spring,
    pub radius: Spring,
    pub shoulder: Spring,
}

impl Animator {
    pub fn new() -> Self {
        Self {
            width: Spring::new(232.0, 155.0, 18.0),
            height: Spring::new(32.0, 155.0, 18.0),
            radius: Spring::new(16.0, 155.0, 25.0),
            shoulder: Spring::new(10.0, 155.0, 25.0),
        }
    }

    pub fn start(&mut self, w: f32, h: f32, r: f32, s: f32) {
        self.width.set_target(w);
        self.height.set_target(h);
        self.radius.set_target(r);
        self.shoulder.set_target(s);
    }

    /// One frame; true while anything is still moving.
    pub fn step(&mut self, dt: f32) -> bool {
        let a = self.width.step(dt);
        let b = self.height.step(dt);
        let c = self.radius.step(dt);
        let d = self.shoulder.step(dt);
        !(a && b && c && d)
    }
}
