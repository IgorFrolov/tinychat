#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MascotEmotion {
    Neutral,
    Happy,
    Laughing,
    Smiling,
    Sad,
    Angry,
    Confused,
    Sleepy,
    InLove,
    Smug,
    DeadInside,
}

impl MascotEmotion {
    pub const ALL: [Self; 11] = [
        Self::Neutral,
        Self::Happy,
        Self::Laughing,
        Self::Smiling,
        Self::Sad,
        Self::Angry,
        Self::Confused,
        Self::Sleepy,
        Self::InLove,
        Self::Smug,
        Self::DeadInside,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Neutral => "Neutral",
            Self::Happy => "Happy",
            Self::Laughing => "Laughing",
            Self::Smiling => "Smiling",
            Self::Sad => "Sad",
            Self::Angry => "Angry",
            Self::Confused => "Confused",
            Self::Sleepy => "Sleepy",
            Self::InLove => "In Love",
            Self::Smug => "Smug",
            Self::DeadInside => "Dead Inside",
        }
    }

    pub const fn face(self) -> &'static str {
        match self {
            Self::Neutral => "(0_0)",
            Self::Happy => "(0‿0)",
            Self::Laughing => "(>▽<)",
            Self::Smiling => "(0ᴗ0)",
            Self::Sad => "(0︵0)",
            Self::Angry => "(>_<)",
            Self::Confused => "(0~0)",
            Self::Sleepy => "(-_-)",
            Self::InLove => "(♥‿♥)",
            Self::Smug => "(0͜0)",
            Self::DeadInside => "(x_x)",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_emotion_has_a_name_and_face() {
        let expected = [
            ("Neutral", "(0_0)"),
            ("Happy", "(0‿0)"),
            ("Laughing", "(>▽<)"),
            ("Smiling", "(0ᴗ0)"),
            ("Sad", "(0︵0)"),
            ("Angry", "(>_<)"),
            ("Confused", "(0~0)"),
            ("Sleepy", "(-_-)"),
            ("In Love", "(♥‿♥)"),
            ("Smug", "(0͜0)"),
            ("Dead Inside", "(x_x)"),
        ];

        assert_eq!(
            MascotEmotion::ALL.map(|emotion| (emotion.name(), emotion.face())),
            expected
        );
    }
}
