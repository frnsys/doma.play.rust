use rand::Rng;
use super::grid::{Position};
use super::city::{City, Unit, Parcel};
use std::cmp::{max};
use std::collections::HashMap;
use rand::seq::SliceRandom;
use linreg::{linear_regression};

static MIN_AREA: f32 = 50.;
static SAMPLE_SIZE: usize = 10;
static TENANT_SAMPLE_SIZE: usize = 30;
static TREND_MONTHS: usize = 12;
static RENT_INCREASE_RATE: f32 = 1.05;
static MOVING_PENALTY: f32 = 10.;

fn distance(a: Position, b: Position) -> f64 {
    (((a.0 - b.0).pow(2) + (a.1 - b.1).pow(2)) as f64).sqrt()
}


#[derive(Debug)]
pub enum AgentType {
    Tenant,
    Landlord
}

#[derive(Debug)]
pub struct Tenant {
    pub id: usize,
    pub income: usize,
    pub unit: Option<usize>,
    pub work: Position,
    pub units: Vec<usize>
}

impl Tenant {
    pub fn step(&mut self, city: &mut City, month: usize, vacant_units: &mut Vec<usize>) {
        let mut reconsider;
        let mut moved = false;
        let mut current_desirability = 0.;
        let mut moving_penalty = MOVING_PENALTY;
        let mut rng = rand::thread_rng();

        match self.unit {
            // If currently w/o home,
            // will always look for a place to move into,
            // with no moving penalty
            None => {
                reconsider = true;
                current_desirability = -1.;
                moving_penalty = 0.;
            },

            // Otherwise, only consider moving
            // between leases or if their current
            // place is no longer affordable
            Some(u_id) => {
                let unit = &mut city.units[u_id];
                let elapsed = if month > unit.lease_month {
                    month - unit.lease_month
                } else {
                    0
                };
                reconsider = elapsed > 0 && elapsed % 12 == 0;
                if !reconsider {
                    // No longer can afford
                    let parcel = &city.parcels[&unit.pos];
                    current_desirability = self.desirability(unit, parcel);
                    if current_desirability == 0. {
                        reconsider = true;
                        unit.tenants.remove(&self.id);
                        vacant_units.push(u_id);
                        self.unit = None;
                    }
                }
            }
        }
        if reconsider && vacant_units.len() > 0 {
            let sample = vacant_units
                .choose_multiple(&mut rng, TENANT_SAMPLE_SIZE);
            let (best_id, best_desirability) = sample.fold((0, 0.), |acc, u_id| {
                let u = &city.units[*u_id];
                let p = &city.parcels[&u.pos];
                if u.vacancies() <= 0 {
                    acc
                } else {
                    let desirability = self.desirability(u, p);
                    if desirability > acc.1 {
                        (*u_id, desirability)
                    } else {
                        acc
                    }
                }
            });
            if best_desirability > 0. && best_desirability - moving_penalty > current_desirability {
                match self.unit {
                    Some(u_id) => {
                        let unit = &mut city.units[u_id];
                        unit.tenants.remove(&self.id);
                        vacant_units.push(u_id);
                    },
                    None => {}
                }

                self.unit = Some(best_id);
                let unit = &mut city.units[best_id];
                unit.tenants.insert(self.id);
                moved = true;
                if unit.vacancies() == 0 {
                    vacant_units.retain(|u_id| *u_id != best_id);
                }
            }
        }
    }

    pub fn desirability(&self, unit: &Unit, parcel: &Parcel) -> f32 {
        // TODO
        // If DOMA is the unit owner,
        // compute rent adjusted for dividends
        // let rent = unit.adjusted_rent(tenants=unit.tenants|set([self]))
        let rent = unit.rent;
        let n_tenants = unit.tenants.len() + 1;
        let rent_per_tenant = max(1, rent/n_tenants);
        if self.income < rent_per_tenant {
            0.
        } else {
            let ratio = (self.income as f32/rent_per_tenant as f32).sqrt();
            let spaciousness = f32::max(unit.area as f32/n_tenants as f32 - MIN_AREA, 0.).powf(1./32.);
            let commute_distance = distance(self.work, unit.pos) as f32;
            let commute: f32 = if commute_distance == 0. {
                1.
            } else {
                1./commute_distance
            };
            ratio * (spaciousness + parcel.desirability + unit.condition + commute)
        }
    }
}

#[derive(Debug)]
pub struct Landlord {
    pub id: usize,
    pub units: Vec<usize>,
    pub maintenance: f32,
    pub rent_obvs: HashMap<usize, Vec<f32>>,
    pub trend_ests: HashMap<usize, f32>,
    pub invest_ests: HashMap<usize, f32>
}

impl Landlord {
    pub fn new(id: usize, neighborhood_ids: Vec<usize>) -> Landlord {
        let mut rent_obvs = HashMap::new();
        let mut trend_ests = HashMap::new();
        let mut invest_ests = HashMap::new();
        for id in neighborhood_ids {
            rent_obvs.insert(id, Vec::new());
            trend_ests.insert(id, 0.);
            invest_ests.insert(id, 0.);
        }

        Landlord {
            id: id,
            units: Vec::new(),
            rent_obvs: rent_obvs,
            trend_ests: trend_ests,
            invest_ests: invest_ests,
            maintenance: 0.1
        }
    }

    pub fn step(&mut self, city: &mut City, month: usize) {
        // Update market estimates
        self.estimate_rents(city);
        self.estimate_trends();

        // Maintenance
        let mut rng = rand::thread_rng();
        for u in &self.units {
            let mut unit = &mut city.units[*u];
            let decay: f32 = rng.gen();
            unit.condition -= decay * 0.1; // TODO deterioration rate based on build year?
            unit.condition += self.maintenance;
            unit.condition = f32::min(f32::max(unit.condition, 0.), 1.);
        }

        // Manage units
        for u in &self.units {
            let mut unit = &mut city.units[*u];
            if unit.tenants.len() == 0 {
                unit.months_vacant += 1;
                if unit.months_vacant % 2 == 0 {
                    // TODO
                    unit.rent = (unit.rent as f32 * 0.98).floor() as usize;
                    // TODO u.maintenance += 0.01
                }
            } else {
                // Year-long leases
                let elapsed = month - unit.lease_month;
                if elapsed > 0 && elapsed % 12 == 0 {
                    // TODO this can be smarter
                    // i.e. depend on gap b/w
                    // current rent and rent estimate/projection
                    unit.rent = (unit.rent as f32 * RENT_INCREASE_RATE).ceil() as usize;
                    // TODO u.maintenance -= 0.01
                }
            }
        }

        // Buy/sells
        // TODO self.make_purchase_offers(sim)
    }

    fn estimate_rents(&mut self, city: &City) {
        let mut rng = rand::thread_rng();
        let mut neighborhoods: HashMap<usize, Vec<f32>> = HashMap::new();
        for u in &self.units {
            let unit = &city.units[*u];
            if unit.tenants.len() > 0 {
                let parcel = &city.parcels[&unit.pos];
                match parcel.neighborhood {
                    Some(neighb_id) => {
                        let n = neighborhoods.entry(neighb_id).or_insert(Vec::new());
                        n.push(unit.rent_per_area());
                    },
                    None => continue
                }
            }
        }

        for (neighb_id, rent_history) in &mut self.rent_obvs {
            let n = neighborhoods.entry(*neighb_id).or_insert(Vec::new());
            let sample = city.units_by_neighborhood[&neighb_id]
                .choose_multiple(&mut rng, SAMPLE_SIZE)
                .map(|u_id| city.units[*u_id].rent_per_area());
            n.extend(sample);
            let max_rent = n.iter().cloned().fold(-1., f32::max);
            rent_history.push(max_rent);
        }
    }

    fn estimate_trends(&mut self) {
        for (neighb_id, rent_history) in &self.rent_obvs {
            if rent_history.len() >= TREND_MONTHS {
                let ys = &rent_history[rent_history.len() - TREND_MONTHS..];
                let xs: Vec<f32> = (0..ys.len()).map(|v| v as f32).collect();
                let (slope, intercept): (f32, f32) = linear_regression(&xs, &ys).unwrap();
                let est_market_rent = (TREND_MONTHS as f32) * slope + intercept;
                self.trend_ests.insert(*neighb_id, est_market_rent);
                self.invest_ests.insert(*neighb_id, est_market_rent - ys.last().unwrap());
            } else {
                continue
            }
        }
    }
}
