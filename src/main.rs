mod qty;
mod tree;
// mod human_format;
use env_logger;
use failure::Error;
use qty::Qty;
use std::str::FromStr;
use itertools::Itertools;

use kube::{
    api::{Api, ListParams},
    client::{APIClient},
    config,
};

#[derive(Debug,Clone,Default)]
struct Location {
    node_name: Option<String>,
    namespace: Option<String>,
    pod_name: Option<String>,
    container_name: Option<String>,
}

#[derive(Debug,Clone)]
struct Resource {
    kind: String,
    quantity: Qty,
    location: Location,
    usage: ResourceUsage,
}

#[derive(Debug,Clone)]
enum ResourceUsage {
    Limit,
    Requested,
    Allocatable,
}

#[derive(Debug,Clone,Default)]
struct QtyOfUsage {
    limit: Qty,
    requested: Qty,
    allocatable: Qty,
}

impl QtyOfUsage {
    pub fn calc_free(&self) -> Qty {
        let total_used = if self.limit > self.requested { &self.limit } else { &self.requested };
        if self.allocatable > *total_used {
            &self.allocatable - total_used
        } else {
            Qty::default()
        }
    }
}
fn sum_by_usage<'a>(rsrcs: &[&Resource]) -> QtyOfUsage {
    rsrcs.iter().fold(QtyOfUsage::default(), |mut acc, v|{
        match &v.usage {
            ResourceUsage::Limit => acc.limit += &v.quantity,
            ResourceUsage::Requested => acc.requested += &v.quantity,
            ResourceUsage::Allocatable => acc.allocatable += &v.quantity,
        };
        acc
    })
}

fn extract_kind(e: &Resource) -> String {
    e.kind.clone()
}

fn extract_node_name(e: &Resource) -> String {
    e.location.node_name.clone().unwrap_or("".to_string())
}

fn make_kind_x_usage(rsrcs: &[Resource]) -> Vec<(Vec<String>, QtyOfUsage)> {
    let group_by_fct: Vec<Box<dyn Fn(&Resource) -> String>> = vec![Box::new(extract_kind), Box::new(extract_node_name)];
    let mut out = make_group_x_usage(&(rsrcs.iter().collect::<Vec<_>>()), &vec![], &group_by_fct, 0);
    out.sort_by_key(|i| i.0.clone());
    out
}

fn make_group_x_usage<F>(rsrcs: &[&Resource], prefix: &[String], group_by_fct: &[F], group_by_depth: usize) -> Vec<(Vec<String>, QtyOfUsage)>
where F: Fn(&Resource) -> String,
{
    // Note: The `&` is significant here, `GroupBy` is iterable
    // only by reference. You can also call `.into_iter()` explicitly.
    let mut out = vec![];
    if let Some(group_by) = group_by_fct.get(group_by_depth) {
        for (key, group) in rsrcs.iter().map(|e| (group_by(e), *e)).into_group_map() {
            let mut key_full = prefix.to_vec();
            key_full.push(key);
            // Check that the sum of each group is +/- 4.
            let children = make_group_x_usage(&group, &key_full, group_by_fct, group_by_depth + 1);
            out.push((key_full, sum_by_usage(&group)));
            out.extend(children);
        }
    }
    // let kg = &rsrcs.into_iter().group_by(|v| v.kind);
    // kg.into_iter().map(|(key, group)|  ).collect()
    out
}

fn collect_from_nodes(client: APIClient, resources: &mut Vec<Resource>) -> Result<(), Error> {
    let api_nodes = Api::v1Node(client);//.within("default");
    let nodes = api_nodes.list(&ListParams::default())?;
    for node in nodes.items {
        let location = Location {
            node_name: Some(node.metadata.name.clone()),
            ..Location::default()
        };
        if let Some(als) = node.status.and_then(|v| v.allocatable) {
            for a in als {
                resources.push(Resource{
                    kind: a.0,
                    usage: ResourceUsage::Allocatable,
                    quantity: Qty::from_str(&(a.1).0)?,
                    location: location.clone(),
                });
            }
        }
    }
    Ok(())
}

fn collect_from_pods(client: APIClient, resources: &mut Vec<Resource>) -> Result<(), Error> {
    let api_pods = Api::v1Pod(client);//.within("default");
    let pods = api_pods.list(&ListParams::default())?;
    for pod in pods.items {
        let node_name = pod.status.and_then(|v| v.nominated_node_name).or(pod.spec.node_name);
        for container in pod.spec.containers {
            let location = Location{
                node_name: node_name.clone(),
                namespace: pod.metadata.namespace.clone(),
                pod_name: Some(pod.metadata.name.clone()),
                container_name: Some(container.name.clone()),
            };
            for requirements in container.resources {
                if let Some(r) = requirements.requests {
                    for request in r {
                        resources.push(Resource{
                            kind: request.0,
                            usage: ResourceUsage::Requested,
                            quantity: Qty::from_str(&(request.1).0)?,
                            location: location.clone(),
                        });
                    }
                }
                if let Some(l) = requirements.limits {
                    for limit in l {
                        resources.push(Resource{
                            kind: limit.0,
                            usage: ResourceUsage::Limit,
                            quantity: Qty::from_str(&(limit.1).0)?,
                            location: location.clone(),
                        });
                    }
                }
            }
        }
    }
    Ok(())
}
fn main() -> Result<(),Error> {
    // std::env::set_var("RUST_LOG", "info,kube=trace");
    env_logger::init();
    let config = config::load_kube_config().expect("failed to load kubeconfig");
    let client = APIClient::new(config);

    let mut resources: Vec<Resource> = vec![];
    collect_from_nodes(client.clone(), &mut resources)?;
    collect_from_pods(client.clone(), &mut resources)?;

    let res = make_kind_x_usage(&resources);
    // display_with_tabwriter(&res);
    display_with_prettytable(&res);
    Ok(())
}

fn display_with_prettytable(data: &[(Vec<String>, QtyOfUsage)]) {
    use prettytable::{Table, row, cell, format};
    // Create the table
    let mut table = Table::new();
    let format = format::FormatBuilder::new()
    // .column_separator('|')
    // .borders('|')
    // .separators(&[format::LinePosition::Top,
    //               format::LinePosition::Bottom],
    //             format::LineSeparator::new('-', '+', '+', '+'))
    .separators(&[], format::LineSeparator::new('-', '+', '+', '+'))
    .padding(1, 1)
    .build();
    table.set_format(format);
    table.set_titles(row![bl->"Resource", br->"Requested", br->"%Requested", br->"Limit",  br->"%Limit", br->"Allocatable", br->"Free"]);
    let prefixes = tree::provide_prefix(data, |parent, item|{
        parent.0.len() + 1 == item.0.len()
    });
    for ((k, qtys), prefix) in data.iter().zip(prefixes.iter()) {
        table.add_row(row![
            &format!("{} {:?}", prefix, k.last().map(|x| x.as_str()).unwrap_or("???")),
            r-> &format!("{}", qtys.requested.adjust_scale()),
            r-> &format!("{:3.0}", qtys.requested.calc_percentage(&qtys.allocatable)),
            r-> &format!("{}", qtys.limit.adjust_scale()),
            r-> &format!("{:3.0}", qtys.limit.calc_percentage(&qtys.allocatable)),
            r-> &format!("{}", qtys.allocatable.adjust_scale()),
            r-> &format!("{}", qtys.calc_free().adjust_scale()),
        ]);
    }

    // Print the table to stdout
    table.printstd();
}
