//! Authoritative ECS columns and immutable fixture infrastructure.
use bevy_ecs::prelude::*;
use smallvec::SmallVec;

pub fn configured_schedule(parallel: bool) -> Schedule {
    let mut schedule = Schedule::default();
    if parallel {
        schedule.set_executor(bevy_ecs::schedule::MultiThreadedExecutor::new());
    } else {
        schedule.set_executor(bevy_ecs::schedule::SingleThreadedExecutor::new());
    }
    schedule
}

#[derive(Component)]
pub struct Id(pub u32);
#[derive(Component)]
pub struct Value {
    pub value: u32,
    pub updates: u32,
}
#[derive(Component, Default)]
pub struct Epoch(pub u32);
#[derive(Component, Default)]
pub struct StageA(pub u32);
#[derive(Component, Default)]
pub struct StageB(pub u32);
#[derive(Component, Default)]
pub struct StageC(pub u32);
#[derive(Component)]
pub struct Words(pub SmallVec<[u32; 4]>);
#[derive(Component)]
pub struct TagD;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Case {
    Dirty,
    Churn,
    Dag,
    Fixed,
    Resize,
}

impl Case {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "dirty_epoch" => Some(Self::Dirty),
            "archetype_churn" => Some(Self::Churn),
            "dependency_dag" => Some(Self::Dag),
            "buffer_fixed" => Some(Self::Fixed),
            "buffer_resize" => Some(Self::Resize),
            _ => None,
        }
    }
    pub fn id(self) -> u32 {
        match self {
            Self::Dirty => 2,
            Self::Churn => 3,
            Self::Dag => 4,
            Self::Fixed => 5,
            Self::Resize => 6,
        }
    }
    pub fn schedule(self) -> bool {
        matches!(self, Self::Dirty | Self::Dag)
    }
}

#[derive(Resource, Clone, Copy)]
pub struct Frame {
    pub tick: u32,
    pub count: u32,
    pub percent: u32,
    pub clustered: bool,
    pub length: usize,
    pub trim: bool,
    pub fork_join: bool,
    pub parallel: bool,
}

pub fn mix(mut value: u32) -> u32 {
    value = (value ^ (value >> 16)).wrapping_mul(0x7feb_352d);
    value = (value ^ (value >> 15)).wrapping_mul(0x846c_a68b);
    value ^ (value >> 16)
}

pub fn update(value: u32, id: u32, tick: u32) -> u32 {
    value
        .wrapping_mul(1_664_525)
        .wrapping_add(1_013_904_223 ^ mix(id.wrapping_add(tick)))
}

pub fn selected(id: u32, frame: Frame) -> bool {
    if !frame.clustered {
        return id % 100 < frame.percent;
    }
    let start = (u64::from(frame.tick) * 997 % u64::from(frame.count)) as u32;
    (u64::from(id) + u64::from(frame.count) - u64::from(start)) % u64::from(frame.count)
        < u64::from(frame.count) * u64::from(frame.percent) / 100
}

pub fn initial_word(id: u32, index: usize) -> u32 {
    mix(id.wrapping_mul(64).wrapping_add(index as u32) ^ 0x6d2b_79f5)
}

pub fn setup(case: Case, frame: Frame) -> (World, Vec<Entity>) {
    let mut world = World::new();
    world.insert_resource(frame);
    let mut entities = Vec::with_capacity(frame.count as usize);
    for id in 0..frame.count {
        let mut entity = world.spawn((
            Id(id),
            Value {
                value: mix(id ^ 0x6d2b_79f5),
                updates: 0,
            },
        ));
        match case {
            Case::Dirty => {
                entity.insert(Epoch::default());
            }
            Case::Dag => {
                entity.insert((StageA::default(), StageB::default(), StageC::default()));
            }
            Case::Fixed | Case::Resize => {
                let length = if case == Case::Fixed { frame.length } else { 4 };
                let words = (0..length).map(|i| initial_word(id, i)).collect();
                entity.insert(Words(words));
            }
            Case::Churn => {}
        }
        entities.push(entity.id());
    }
    (world, entities)
}
