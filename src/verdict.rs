// Turns raw measurements into a plain-language diagnosis.

use crate::ping::Summary;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    Measuring,
    Good,
    Fair,
    Poor,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    None,
    NoNetwork,
    NoRouter,
    NoInternet,
    Down,
    Local,
    WeakWifi,
    Beyond,
    Unstable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Diagnosis {
    pub health: Health,
    pub cause: Cause,
}

pub struct Inputs {
    pub has_network: bool,
    // None when no router (default gateway) is known.
    pub router: Option<Summary>,
    pub internet: Option<Summary>,
    pub wifi_dbm: Option<i32>,
}

pub fn diagnose(i: &Inputs) -> Diagnosis {
    let down = |cause| Diagnosis { health: Health::Down, cause };
    let measuring = Diagnosis { health: Health::Measuring, cause: Cause::None };
    if !i.has_network {
        return down(Cause::NoNetwork);
    }
    let Some(net) = i.internet else { return measuring };
    if net.lost_streak >= 3 {
        return down(match i.router {
            Some(r) if r.replies > 0 && r.lost_streak == 0 => Cause::NoInternet,
            Some(r) if r.replies == 0 && r.done >= 3 => Cause::NoRouter,
            _ => Cause::Down,
        });
    }
    if net.done < 4 || net.replies == 0 {
        return measuring;
    }
    let lat = net.median.unwrap_or(0.0);
    let jit = net.jitter.unwrap_or(0.0);
    let loss = net.loss_or_zero();

    let health = if loss >= 2.0 || lat >= 150.0 || jit >= 30.0 {
        Health::Poor
    } else if loss >= 0.5 || lat >= 80.0 || jit >= 15.0 {
        Health::Fair
    } else {
        Health::Good
    };
    if health == Health::Good {
        return Diagnosis { health, cause: Cause::None };
    }

    // Blame the router only when its delay is a big share of the total.
    let local_bad = i.router.is_some_and(|r| {
        let (rl, rj) = (r.median.unwrap_or(0.0), r.jitter.unwrap_or(0.0));
        r.replies > 0
            && ((rl >= 15.0 && rl >= 0.4 * lat)
                || (rj >= 10.0 && rj >= 0.5 * jit)
                || (r.loss_or_zero() >= 2.0 && loss >= 1.0))
    });
    let router_fine = i.router.is_some_and(|r| {
        let rl = r.median.unwrap_or(0.0);
        r.replies >= 3 && r.loss_or_zero() < 1.0 && r.jitter.unwrap_or(0.0) < 10.0 && (rl < 10.0 || rl < 0.4 * lat)
    });
    let cause = if i.wifi_dbm.is_some_and(|x| x < -75) {
        Cause::WeakWifi
    } else if local_bad {
        Cause::Local
    } else if router_fine {
        Cause::Beyond
    } else {
        Cause::Unstable
    };
    Diagnosis { health, cause }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(median: f64, jitter: f64, loss: f64) -> Summary {
        let done = 60;
        let replies = done - (loss * done as f64 / 100.0).round() as usize;
        Summary {
            last: Some(median),
            median: Some(median),
            jitter: Some(jitter),
            loss: Some(loss),
            done,
            replies,
            lost_streak: 0,
        }
    }

    fn dead() -> Summary {
        Summary { done: 5, lost_streak: 5, loss: Some(100.0), ..Default::default() }
    }

    fn run(router: Option<Summary>, internet: Summary, dbm: Option<i32>) -> Diagnosis {
        diagnose(&Inputs { has_network: true, router, internet: Some(internet), wifi_dbm: dbm })
    }

    fn pair(d: Diagnosis) -> (Health, Cause) {
        (d.health, d.cause)
    }

    #[test]
    fn healthy_line() {
        assert_eq!(pair(run(Some(s(2.0, 0.5, 0.0)), s(18.0, 2.0, 0.0), Some(-55))), (Health::Good, Cause::None));
    }

    #[test]
    fn blames_wifi_when_router_is_slow_too() {
        assert_eq!(pair(run(Some(s(40.0, 25.0, 3.0)), s(60.0, 30.0, 3.0), Some(-60))), (Health::Poor, Cause::Local));
    }

    #[test]
    fn blames_weak_signal_first() {
        assert_eq!(pair(run(Some(s(5.0, 3.0, 0.0)), s(40.0, 20.0, 1.0), Some(-80))), (Health::Fair, Cause::WeakWifi));
    }

    #[test]
    fn blames_provider_when_router_is_fine() {
        assert_eq!(pair(run(Some(s(2.0, 0.5, 0.0)), s(90.0, 20.0, 3.0), None)), (Health::Poor, Cause::Beyond));
    }

    #[test]
    fn router_that_ignores_pings_is_not_blamed() {
        assert_eq!(pair(run(Some(s(30.0, 2.0, 0.0)), s(110.0, 5.0, 0.0), None)), (Health::Fair, Cause::Beyond));
    }

    #[test]
    fn outages() {
        assert_eq!(run(Some(s(2.0, 0.5, 0.0)), dead(), None).cause, Cause::NoInternet);
        let quiet_router = Summary { done: 5, replies: 0, lost_streak: 5, loss: Some(100.0), ..Default::default() };
        assert_eq!(run(Some(quiet_router), dead(), None).cause, Cause::NoRouter);
        let d = diagnose(&Inputs { has_network: false, router: None, internet: None, wifi_dbm: None });
        assert_eq!(pair(d), (Health::Down, Cause::NoNetwork));
    }

    #[test]
    fn waits_for_data() {
        let few = Summary { done: 2, replies: 2, median: Some(10.0), ..Default::default() };
        assert_eq!(run(None, few, None).health, Health::Measuring);
        let d = diagnose(&Inputs { has_network: true, router: None, internet: None, wifi_dbm: None });
        assert_eq!(d.health, Health::Measuring);
    }
}
