//! plato-room-scheduler — Room Training Scheduler
//!
//! Decides WHEN to train rooms based on temperature, priority, and resources.
//! Cold rooms don't train. Warm rooms schedule background. Hot rooms get priority.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TrainPriority {
    Skip,
    Background,
    Normal,
    High,
    Critical,
}

#[derive(Debug, Clone)]
pub struct SchedulableRoom {
    pub id: String,
    pub tile_count: usize,
    pub domain: String,
    pub last_trained_at: u64,
    pub training_count: u32,
    pub avg_confidence: f64,
    pub has_new_tiles: bool,
    pub priority_override: Option<TrainPriority>,
}

impl SchedulableRoom {
    pub fn new(id: &str, tile_count: usize, domain: &str) -> Self {
        SchedulableRoom {
            id: id.into(), tile_count, domain: domain.into(),
            last_trained_at: 0, training_count: 0, avg_confidence: 0.5,
            has_new_tiles: false, priority_override: None,
        }
    }
    pub fn temperature(&self) -> RoomTemp {
        match self.tile_count {
            0..=49 => RoomTemp::Cold,
            50..=499 => RoomTemp::Warm,
            500..=999 => RoomTemp::Hot,
            _ => RoomTemp::Crystallized,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RoomTemp { Cold, Warm, Hot, Crystallized }

#[derive(Debug, Clone)]
pub struct TrainBudget {
    pub max_concurrent: usize,
    pub available_minutes: u64,
}
impl Default for TrainBudget {
    fn default() -> Self { TrainBudget { max_concurrent: 2, available_minutes: 120 } }
}

#[derive(Debug, Clone)]
pub struct TrainJob {
    pub room_id: String,
    pub priority: TrainPriority,
    pub estimated_minutes: u64,
    pub reason: String,
}

pub struct RoomScheduler;

impl RoomScheduler {
    pub fn classify(room: &SchedulableRoom, now: u64) -> TrainPriority {
        if let Some(p) = room.priority_override { return p; }
        match room.temperature() {
            RoomTemp::Cold => TrainPriority::Skip,
            RoomTemp::Warm => if room.has_new_tiles { TrainPriority::Normal } else { TrainPriority::Background },
            RoomTemp::Hot => TrainPriority::High,
            RoomTemp::Crystallized => {
                let hours = now.saturating_sub(room.last_trained_at) / 3600;
                if hours > 24 { TrainPriority::Normal } else { TrainPriority::Skip }
            }
        }
    }

    pub fn schedule(rooms: &[SchedulableRoom], budget: &TrainBudget, now: u64) -> Vec<TrainJob> {
        let mut classified: Vec<(TrainPriority, &SchedulableRoom)> = rooms.iter()
            .filter_map(|r| {
                let p = Self::classify(r, now);
                if p == TrainPriority::Skip { None } else { Some((p, r)) }
            }).collect();
        classified.sort_by(|a, b| b.0.cmp(&a.0));

        let mut jobs = Vec::new();
        let mut used = 0u64;
        for (priority, room) in classified {
            if jobs.len() >= budget.max_concurrent { break; }
            let est = Self::estimate(room);
            if used + est > budget.available_minutes { continue; }
            let reason = match priority {
                TrainPriority::Critical => "critical override".into(),
                TrainPriority::High => format!("hot room ({} tiles)", room.tile_count),
                TrainPriority::Normal => format!("warm+new ({} tiles)", room.tile_count),
                TrainPriority::Background => format!("idle cycle ({} tiles)", room.tile_count),
                TrainPriority::Skip => unreachable!(),
            };
            jobs.push(TrainJob { room_id: room.id.clone(), priority, estimated_minutes: est, reason });
            used += est;
        }
        jobs
    }

    fn estimate(room: &SchedulableRoom) -> u64 { (room.tile_count as u64 * 45) / 500 }

    pub fn needs_retraining(room: &SchedulableRoom, now: u64) -> bool {
        match room.temperature() {
            RoomTemp::Cold => false,
            RoomTemp::Warm => room.has_new_tiles,
            RoomTemp::Hot => now.saturating_sub(room.last_trained_at) / 3600 > 6,
            RoomTemp::Crystallized => now.saturating_sub(room.last_trained_at) / 3600 > 48,
        }
    }
}

fn now() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_cold() {
        assert_eq!(RoomScheduler::classify(&SchedulableRoom::new("c", 10, "x"), now()), TrainPriority::Skip);
    }

    #[test]
    fn test_classify_warm_new() {
        let mut r = SchedulableRoom::new("w", 100, "x"); r.has_new_tiles = true;
        assert_eq!(RoomScheduler::classify(&r, now()), TrainPriority::Normal);
    }

    #[test]
    fn test_classify_warm_idle() {
        assert_eq!(RoomScheduler::classify(&SchedulableRoom::new("w", 100, "x"), now()), TrainPriority::Background);
    }

    #[test]
    fn test_classify_hot() {
        assert_eq!(RoomScheduler::classify(&SchedulableRoom::new("h", 600, "x"), now()), TrainPriority::High);
    }

    #[test]
    fn test_classify_crystallized_fresh() {
        let mut r = SchedulableRoom::new("cr", 1500, "x"); r.last_trained_at = now();
        assert_eq!(RoomScheduler::classify(&r, now()), TrainPriority::Skip);
    }

    #[test]
    fn test_classify_crystallized_stale() {
        let mut r = SchedulableRoom::new("cr", 1500, "x"); r.last_trained_at = now() - 48*3600 - 1;
        assert_eq!(RoomScheduler::classify(&r, now()), TrainPriority::Normal);
    }

    #[test]
    fn test_override() {
        let mut r = SchedulableRoom::new("c", 10, "x"); r.priority_override = Some(TrainPriority::Critical);
        assert_eq!(RoomScheduler::classify(&r, now()), TrainPriority::Critical);
    }

    #[test]
    fn test_schedule_skips_cold() {
        let rooms = vec![
            SchedulableRoom::new("cold", 10, "x"),
            SchedulableRoom::new("warm", 100, "x"),
            SchedulableRoom::new("hot", 600, "x"),
        ];
        let jobs = RoomScheduler::schedule(&rooms, &TrainBudget::default(), now());
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].priority, TrainPriority::High);
    }

    #[test]
    fn test_schedule_respects_concurrent() {
        let rooms: Vec<SchedulableRoom> = (0..5).map(|i| {
            let mut r = SchedulableRoom::new(&format!("h{}", i), 600, "x"); r.has_new_tiles = true; r
        }).collect();
        let jobs = RoomScheduler::schedule(&rooms, &TrainBudget { max_concurrent: 2, available_minutes: 9999 }, now());
        assert_eq!(jobs.len(), 2);
    }

    #[test]
    fn test_schedule_sorts_priority() {
        let rooms = vec![
            SchedulableRoom::new("bg", 100, "x"),
            SchedulableRoom::new("hi", 600, "x"),
        ];
        let jobs = RoomScheduler::schedule(&rooms, &TrainBudget::default(), now());
        assert_eq!(jobs[0].room_id, "hi");
    }

    #[test]
    fn test_needs_retraining_hot() {
        let mut r = SchedulableRoom::new("h", 600, "x"); r.last_trained_at = now() - 7*3600;
        assert!(RoomScheduler::needs_retraining(&r, now()));
    }

    #[test]
    fn test_needs_retraining_cold() {
        assert!(!RoomScheduler::needs_retraining(&SchedulableRoom::new("c", 10, "x"), now()));
    }

    #[test]
    fn test_estimate() {
        assert_eq!(RoomScheduler::estimate(&SchedulableRoom::new("x", 500, "x")), 45);
    }
}
