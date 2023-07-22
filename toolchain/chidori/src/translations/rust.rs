use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::marker::PhantomData;
use anyhow::Error;
use futures::StreamExt;
use log::{debug, info};
use once_cell::sync::OnceCell;
use tokio::runtime::Runtime;
use prompt_graph_core::build_runtime_graph::graph_parse::{CleanedDefinitionGraph, CleanIndividualNode, construct_query_from_output_type, derive_for_individual_node};
use prompt_graph_core::graph_definition::{create_code_node, create_custom_node, create_prompt_node, create_vector_memory_node, SourceNodeType};
use prompt_graph_core::proto2::{ChangeValue, ChangeValueWithCounter, Empty, ExecutionStatus, File, FileAddressedChangeValueWithCounter, FilteredPollNodeWillExecuteEventsRequest, Item, ListBranchesRes, Path, Query, QueryAtFrame, QueryAtFrameResponse, RequestAckNodeWillExecuteEvent, RequestAtFrame, RequestFileMerge, RequestListBranches, RequestNewBranch, RequestOnlyId, RespondPollNodeWillExecuteEvents, SerializedValue, SerializedValueArray, SerializedValueObject};
use prompt_graph_core::proto2::execution_runtime_client::ExecutionRuntimeClient;
use prompt_graph_core::proto2::serialized_value::Val;
use prompt_graph_exec::tonic_runtime::run_server;
use neon_serde3;
use serde::{Deserialize, Serialize};
use tonic::Status;

async fn get_client(url: String) -> Result<ExecutionRuntimeClient<tonic::transport::Channel>, tonic::transport::Error> {
    ExecutionRuntimeClient::connect(url.clone()).await
}

struct Chidori {
    file_id: String,
    current_head: u64,
    current_branch: u64,
    url: String
}

impl Chidori {

    fn new(file_id: String, url: String) -> Self {
        if !url.contains("://") {
            panic!("Invalid url, must include protocol");
        }
        // let api_token = cx.argument_opt(2)?.value(&mut cx);
        debug!("Creating new Chidori instance with file_id={}, url={}, api_token={:?}", file_id, url, "".to_string());
        Chidori {
            file_id,
            current_head: 0,
            current_branch: 0,
            url,
        }
    }

    async fn start_server(&self, file_path: Option<String>) -> anyhow::Result<()> {
        let url_server = self.url.clone();
        std::thread::spawn(move || {
            let result = run_server(url_server, file_path);
            match result {
                Ok(_) => {
                    println!("Server exited");
                },
                Err(e) => {
                    println!("Error running server: {}", e);
                },
            }
        });

        let url = self.url.clone();
        loop {
            match get_client(url.clone()).await {
                Ok(connection) => {
                    eprintln!("Connection successfully established {:?}", &url);
                    return Ok(());
                },
                Err(e) => {
                    eprintln!("Error connecting to server: {} with Error {}. Retrying...", &url, &e.to_string());
                    std::thread::sleep(std::time::Duration::from_millis(1000));
                }
            }
        }
    }

    async fn play(&self, branch: u64, frame: u64) -> anyhow::Result<ExecutionStatus> {
        let file_id = self.file_id.clone();
        let url = self.url.clone();
        let mut client = get_client(url).await?;
        let result = client.play(RequestAtFrame {
            id: file_id,
            frame,
            branch,
        }).await?;
        Ok(result.into_inner())
    }

    async fn pause(&self, frame: u64) -> anyhow::Result<ExecutionStatus> {
        let file_id = self.file_id.clone();
        let url = self.url.clone();
        let branch = self.current_branch.clone();

        let mut client = get_client(url).await?;
        let result = client.pause(RequestAtFrame {
            id: file_id,
            frame,
            branch,
        }).await?;
        Ok(result.into_inner())
    }

    async fn query( &self, query: String, branch: u64, frame: u64, ) -> anyhow::Result<QueryAtFrameResponse> {
        let file_id = self.file_id.clone();
        let url = self.url.clone();
        let mut client = get_client(url).await?;
        let result = client.run_query(QueryAtFrame {
            id: file_id,
            query: Some(Query {
                query: Some(query)
            }),
            frame,
            branch,
        }).await?;
        Ok(result.into_inner())
    }

    async fn list_branches( &self) -> anyhow::Result<ListBranchesRes> {
        let file_id = self.file_id.clone();
        let url = self.url.clone();
        let mut client = get_client(url).await?;
        let result = client.list_branches(RequestListBranches {
            id: file_id,
        }).await?;
        Ok(result.into_inner())
    }

    async fn display_graph_structure( &self, query: String, branch: u64, frame: u64, ) -> anyhow::Result<String> {
        let file_id = self.file_id.clone();
        let url = self.url.clone();
        let mut client = get_client(url).await?;
        let file = client.current_file_state(RequestOnlyId {
            id: file_id,
            branch
        }).await?;
        let mut file = file.into_inner();
        let mut g = CleanedDefinitionGraph::zero();
        g.merge_file(&mut file).unwrap();
        Ok(g.get_dot_graph())
    }

    async fn list_registered_graphs(&self) -> anyhow::Result<()> {
        let file_id = self.file_id.clone();
        let url = self.url.clone();
        let mut client = get_client(url).await?;
        let resp = client.list_registered_graphs(Empty { }).await?;
        let mut stream = resp.into_inner();
        while let Some(x) = stream.next().await {
            // callback.call(py, (x,), None);
            info!("Registered Graph = {:?}", x);
        };
        Ok(())
    }

//
//     // TODO: need to figure out how to handle callbacks
//     // fn list_input_proposals<'a>(
//     //     mut self_: PyRefMut<'_, Self>,
//     //     py: Python<'a>,
//     //     callback: PyObject
//     // ) -> PyResult<&'a PyAny> {
//     //     let file_id = self_.file_id.clone();
//     //     let url = self_.url.clone();
//     //     let branch = self_.current_branch;
//     //     pyo3_asyncio::tokio::future_into_py(py, async move {
//     //         let mut client = get_client(url).await?;
//     //         let resp = client.list_input_proposals(RequestOnlyId {
//     //             id: file_id,
//     //             branch,
//     //         }).await.map_err(PyErrWrapper::from)?;
//     //         let mut stream = resp.into_inner();
//     //         while let Some(x) = stream.next().await {
//     //             // callback.call(py, (x,), None);
//     //             info!("InputProposals = {:?}", x);
//     //         };
//     //         Ok(())
//     //     })
//     // }
//
//     // fn respond_to_input_proposal(mut self_: PyRefMut<'_, Self>) -> PyResult<()> {
//     //     Ok(())
//     // }
//
//     // TODO: need to figure out how to handle callbacks
//     // fn list_change_events<'a>(
//     //     mut self_: PyRefMut<'_, Self>,
//     //     py: Python<'a>,
//     //     callback: PyObject
//     // ) -> PyResult<&'a PyAny> {
//     //     let file_id = self_.file_id.clone();
//     //     let url = self_.url.clone();
//     //     let branch = self_.current_branch;
//     //     pyo3_asyncio::tokio::future_into_py(py, async move {
//     //         let mut client = get_client(url).await?;
//     //         let resp = client.list_change_events(RequestOnlyId {
//     //             id: file_id,
//     //             branch,
//     //         }).await.map_err(PyErrWrapper::from)?;
//     //         let mut stream = resp.into_inner();
//     //         while let Some(x) = stream.next().await {
//     //             Python::with_gil(|py| pyo3_asyncio::tokio::into_future(callback.as_ref(py).call((x.map(ChangeValueWithCounterWrapper).map_err(PyErrWrapper::from)?,), None)?))?
//     //                 .await?;
//     //         };
//     //         Ok(())
//     //     })
//     // }
//
//
//
//     // TODO: this should accept an "Object" instead of args
//     // TODO: nodes that are added should return a clean definition of what their addition looks like
//     // TODO: adding a node should also display any errors


    async fn poll_local_code_node_execution(&self) -> anyhow::Result<RespondPollNodeWillExecuteEvents> {
        let file_id = self.file_id.clone();
        let url = self.url.clone();
        let mut client = get_client(url).await?;
        let req = FilteredPollNodeWillExecuteEventsRequest { id: file_id.clone() };
        let result = client.poll_node_will_execute_events(req).await?;
        Ok(result.into_inner())
    }

    async fn ack_local_code_node_execution(&self, branch: u64, counter : u64) -> anyhow::Result<ExecutionStatus> {
        let file_id = self.file_id.clone();
        let url = self.url.clone();
        let mut client = get_client(url).await?;
        let result = client.ack_node_will_execute_event(RequestAckNodeWillExecuteEvent {
            id: file_id.clone(),
            branch,
            counter,
        }).await?;
        Ok(result.into_inner())
    }

    async fn respond_local_code_node_execution(&self, branch: u64, counter: u64, node_name: String, response: Vec<ChangeValue>) -> anyhow::Result<ExecutionStatus> {
        let file_id = self.file_id.clone();
        let url = self.url.clone();

        // TODO: need parent counters from the original change
        // TODO: need source node
        let mut client = get_client(url).await?;

        // TODO: need to add the output table paths to these
        // TODO: this needs to look more like a real change
        Ok(client.push_worker_event(FileAddressedChangeValueWithCounter {
            branch,
            counter,
            node_name,
            id: file_id.clone(),
            change: Some(ChangeValueWithCounter {
                filled_values: response,
                parent_monotonic_counters: vec![],
                monotonic_counter: counter,
                branch,
                source_node: "".to_string(),
            })
        }).await?.into_inner())
    }
}


#[derive(serde::Serialize, serde::Deserialize)]
struct PromptNodeCreateOpts {
    name: String,
    queries: Option<Vec<String>>,
    output_tables: Option<Vec<String>>,
    template: String,
    model: Option<String>
}


#[derive(serde::Serialize, serde::Deserialize)]
struct CustomNodeCreateOpts {
    name: String,
    queries: Option<Vec<String>>,
    output_tables: Option<Vec<String>>,
    output: Option<String>,
    node_type_name: String
}

#[derive(serde::Serialize, serde::Deserialize)]
struct DenoCodeNodeCreateOpts {
    name: String,
    queries: Option<Vec<String>>,
    output_tables: Option<Vec<String>>,
    output: Option<String>,
    code: String,
    is_template: Option<bool>
}

#[derive(serde::Serialize, serde::Deserialize)]
struct VectorMemoryNodeCreateOpts {
    name: String,
    queries: Option<Vec<String>>,
    output_tables: Option<Vec<String>>,
    output: Option<String>,
    template: Option<String>, // TODO: default is the contents of the query
    action: Option<String>, // TODO: default WRITE
    embedding_model: Option<String>, // TODO: default TEXT_EMBEDDING_ADA_002
    db_vendor: Option<String>, // TODO: default QDRANT
    collection_name: String,
}


fn remap_queries(queries: Option<Vec<String>>) -> Vec<Option<String>> {
    let queries: Vec<Option<String>> = if let Some(queries) = queries {
        queries.into_iter().map(|q| {
            if q == "None".to_string() {
                None
            } else {
                Some(q)
            }
        }).collect()
    } else {
        vec![]
    };
    queries
}

struct GraphBuilder {
    clean_graph: CleanedDefinitionGraph,
}

impl GraphBuilder {
    fn prompt_node(&mut self, arg: PromptNodeCreateOpts) -> anyhow::Result<NodeHandle> {
        let node = create_prompt_node(
            arg.name,
            remap_queries(arg.queries),
            arg.template,
            arg.model.unwrap_or("GPT_3_5_TURBO".to_string()),
            arg.output_tables.unwrap_or(vec![]))?;
        self.clean_graph.merge_file(&File { nodes: vec![node.clone()], ..Default::default() })?;
        Ok(NodeHandle::from(node)?)
    }

    fn custom_node(&mut self, arg: CustomNodeCreateOpts) -> anyhow::Result<NodeHandle> {
        let node = create_custom_node(
            arg.name,
            remap_queries(arg.queries.clone()),
            arg.output.unwrap_or("type O {}".to_string()),
            arg.node_type_name,
            arg.output_tables.unwrap_or(vec![])
        );
        self.clean_graph.merge_file(&File { nodes: vec![node.clone()], ..Default::default() })?;
        Ok(NodeHandle::from(node)?)
    }


    fn deno_code_node(&mut self, arg: DenoCodeNodeCreateOpts) -> anyhow::Result<NodeHandle> {
        let node = create_code_node(
            arg.name,
            remap_queries(arg.queries.clone()),
            arg.output.unwrap_or("type O {}".to_string()),
            SourceNodeType::Code("DENO".to_string(), arg.code, arg.is_template.unwrap_or(false)),
            arg.output_tables.unwrap_or(vec![])
        );
        self.clean_graph.merge_file(&File { nodes: vec![node.clone()], ..Default::default() })?;
        Ok(NodeHandle::from(node)?)
    }


    fn vector_memory_node(&mut self, arg: VectorMemoryNodeCreateOpts) -> anyhow::Result<NodeHandle> {
        let node = create_vector_memory_node(
            arg.name,
            remap_queries(arg.queries.clone()),
            arg.output.unwrap_or("type O {}".to_string()),
            arg.action.unwrap_or("READ".to_string()),
            arg.embedding_model.unwrap_or("TEXT_EMBEDDING_ADA_002".to_string()),
            arg.template.unwrap_or("".to_string()),
            arg.db_vendor.unwrap_or("QDRANT".to_string()),
            arg.collection_name,
            arg.output_tables.unwrap_or(vec![])
        )?;
        self.clean_graph.merge_file(&File { nodes: vec![node.clone()], ..Default::default() })?;
        Ok(NodeHandle::from(node)?)
    }
//
//
//     //
//     // fn observation_node(mut self_: PyRefMut<'_, Self>, name: String, query_def: Option<String>, template: String, model: String) -> PyResult<()> {
//     //     let file_id = self_.file_id.clone();
//     //     let node = create_observation_node(
//     //         "".to_string(),
//     //         None,
//     //         "".to_string(),
//     //     );
//     //     executor::block_on(self_.client.merge(RequestFileMerge {
//     //         id: file_id,
//     //         file: Some(File {
//     //             nodes: vec![node],
//     //             ..Default::default()
//     //         }),
//     //         branch: 0,
//     //     }));
//     //     Ok(())
//     // }

    //     // TODO: need to figure out passing a buffer of bytes
//     // TODO: nodes that are added should return a clean definition of what their addition looks like
//     // TODO: adding a node should also display any errors
//     /// x = None
//     /// with open("/Users/coltonpierson/Downloads/files_and_dirs.zip", "rb") as zip_file:
//     ///     contents = zip_file.read()
//     ///     x = await p.load_zip_file("LoadZip", """ output: String """, contents)
//     /// x
//     // #[pyo3(signature = (name=String::new(), output_tables=vec![], output=String::new(), bytes=vec![]))]
//     // fn load_zip_file<'a>(
//     //     mut self_: PyRefMut<'_, Self>,
//     //     py: Python<'a>,
//     //     name: String,
//     //     output_tables: Vec<String>,
//     //     output: String,
//     //     bytes: Vec<u8>
//     // ) -> PyResult<&'a PyAny> {
//     //     let file_id = self_.file_id.clone();
//     //     let url = self_.url.clone();
//     //     pyo3_asyncio::tokio::future_into_py(py, async move {
//     //         let node = create_loader_node(
//     //             name,
//     //             vec![],
//     //             output,
//     //             LoadFrom::ZipfileBytes(bytes),
//     //             output_tables
//     //         );
//     //         Ok(push_file_merge(&url, &file_id, node).await?)
//     //     })
//     // }

    async fn commit(&self, file_id: String, url: String, branch: u64) -> anyhow::Result<ExecutionStatus> {
        let mut client = get_client(url.clone()).await?;
        let nodes = self.clean_graph.node_by_name.clone().into_values().collect();
        Ok(client.merge(RequestFileMerge {
            id: file_id.clone(),
            file: Some(File { nodes, ..Default::default() }),
            branch: 0,
        }).await.map(|x| x.into_inner())?)
    }
}


// Node handle
#[derive(Clone)]
pub struct NodeHandle {
    node: Item,
    indiv: CleanIndividualNode
}

impl NodeHandle {
    fn from(node: Item) -> anyhow::Result<NodeHandle> {
        let indiv = derive_for_individual_node(&node)?;
        Ok(NodeHandle {
            node,
            indiv
        })
    }
}


impl NodeHandle {
    fn get_name(&self) -> String {
        self.node.core.as_ref().unwrap().name.clone()
    }

    pub fn run_when(&mut self, other_node: &NodeHandle) -> anyhow::Result<bool> {
        let queries = &mut self.node.core.as_mut().unwrap().queries;
        let q = construct_query_from_output_type(
            &other_node.get_name(),
            &other_node.get_name(),
            &self.indiv.output_path
        ).unwrap();
        queries.push(Query { query: Some(q)});
        Ok(true)
    }


    pub async fn query(&self, file_id: String, url: String, branch: u64, frame: u64) -> anyhow::Result<HashMap<String, SerializedValue>> {
        let name = &self.node.core.as_ref().unwrap().name;
        let query = construct_query_from_output_type(&name, &name, &self.indiv.output_path).unwrap();
        let mut client = get_client(url).await?;
        let result = client.run_query(QueryAtFrame {
            id: file_id,
            query: Some(Query {
                query: Some(query)
            }),
            frame,
            branch,
        }).await?;
        let res = result.into_inner();
        let mut obj = HashMap::new();
        for value in res.values.iter() {
            let c = value.change_value.as_ref().unwrap();
            let k = c.path.as_ref().unwrap().address.join(":");
            let v = c.value.as_ref().unwrap().clone();
            obj.insert(k, v).unwrap();
        }
        Ok(obj)
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_graph() {
    }
}