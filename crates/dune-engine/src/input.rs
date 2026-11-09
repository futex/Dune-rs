#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Enter,
    Esc,
    Tab,
    Space,
    F1,
    Char(char),
}

#[derive(Default, Debug, Clone)]
pub struct Input {
    pub keys: Vec<Key>,
    /// Mouse position in 320x200 screen space.
    pub mouse: (i32, i32),
    pub clicked: bool,
    /// Left button currently down (zone auto-repeat).
    pub held: bool,
    pub right_clicked: bool,
}

impl Input {
    pub fn pressed(&self, k: Key) -> bool {
        self.keys.contains(&k)
    }
    pub fn any(&self) -> bool {
        !self.keys.is_empty() || self.clicked || self.right_clicked
    }
}
