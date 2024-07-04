use std::error::Error;
use std::io;
use std::net::SocketAddr;

use hickory_client::op::Message;
use tokio::net::UdpSocket;
use tokio::time::Duration;
use ttlhashmap::TtlHashMap;

use crate::dns_client::DnsClient;

pub struct DnsServer {
    pub socket: UdpSocket,
    pub buf: Vec<u8>,
    pub to_send: Option<(usize, SocketAddr)>,
    pub dns_clients: Vec<DnsClient>,
}

impl DnsServer {
    pub async fn new(
        upstream_dns_servers: Vec<SocketAddr>,
        socket: UdpSocket,
        buf: Vec<u8>,
    ) -> Result<Self, Box<dyn Error>> {
        let mut dns_clients = Vec::new();
        for server in upstream_dns_servers {
            let dns_client = DnsClient::new(server)
                .await
                .expect(format!("Failed to create DNS client for server: {}", server).as_str());
            dns_clients.push(dns_client);
        }

        Ok(Self {
            socket,
            buf,
            to_send: None,
            dns_clients,
        })
    }

    pub async fn run(&mut self) -> Result<(), io::Error> {
        // hashmap which saves responses within ttl to cache
        let ttl_dns_hashmap: TtlHashMap<Vec<u8>, Vec<u8>> =
            TtlHashMap::new(Duration::from_secs(100));
        loop {
            if let Some((_size, peer)) = self.to_send {
                let query_msg = match Message::from_vec(&self.buf) {
                    Ok(msg) => msg,
                    Err(e) => {
                        tracing::error!("Failed to parse query message: {}", e);
                        continue;
                    }
                };

                tracing::info!("Query message: {}", query_msg.clone());

                // Check if the query is in the cache
                // TODO: Implement cache

                // Query the upstream servers asynchronously
                let futures = self.dns_clients.iter().map(|client| {
                    let query_msg_clone = query_msg.clone();
                    async move {
                        let response = client.send_query(query_msg_clone).await;
                        match response {
                            Some(response) => Some(response),
                            None => None,
                        }
                    }
                });

                let results = futures::future::join_all(futures).await;
                let results = results
                    .into_iter()
                    .filter_map(|result| result)
                    .collect::<Vec<Message>>();

                // Check if any of the responses are valid
                if results.len() > 0 {
                    let mut response_message = results[0].clone();
                    // tracing::info!("Received response from upstream servers: {:?}", results);
                    response_message.set_header(
                        *response_message
                            .header()
                            .clone()
                            .set_id(query_msg.header().id()),
                    );
                    if let Ok(response_vec) = response_message.to_vec() {
                        // Send the response to the client
                        // TODO: Implement cache
                        let amt = self.socket.send_to(&response_vec, &peer).await?;
                        tracing::info!(
                            "Sent {} bytes to {}, dns msg id: {}",
                            amt,
                            peer,
                            query_msg.header().id()
                        );
                    } else {
                        // failure to serialize response message
                        // TODO: respond with no answer
                        tracing::error!("Failed to serialize response message");
                    }
                } else {
                    // failure to get valid response from upstream servers
                    // TODO: respond with no answer
                    tracing::error!("Failed to get valid response from upstream servers");
                }
            }
            self.to_send = Some(self.socket.recv_from(&mut self.buf).await?);
        }
    }
}