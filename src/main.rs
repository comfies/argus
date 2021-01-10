use async_std::{io, task};
use futures::{future, prelude::*};
use libp2p::{
    Multiaddr,
    PeerId,
    Swarm,
    NetworkBehaviour,
    identity,
    floodsub::{self, Floodsub, FloodsubEvent},
    mdns::{Mdns, MdnsEvent},
    swarm::NetworkBehaviourEventProcess
};
use std::{error::Error, task::{Context, Poll}};

fn main() -> Result<(), Box<dyn Error>> {
    // Start logging everything that's happening
    env_logger::init();
    
    // Generate a unique, random peer ID
    let local_key = identity::Keypair::generate_ed25519();
    let local_peer_id = PeerId::from(local_key.public());
    println!("Local peer id: {:?}", local_peer_id);

    // Setup a transport to actually connect with the world
    let transport = libp2p::build_development_transport(local_key)?;
    
    // Set the topic to "chat" to find others on the same topic
    let floodsub_topic = floodsub::Topic::new("chat");
    
    // Combine the floodsub and mDNS protocols into one behaviour
    #[derive(NetworkBehaviour)]
    struct MyBehaviour {
        floodsub: Floodsub, // For the actual messaging
        mdns: Mdns, // For tracking active users
        
        #[behaviour(ignore)]
        #[allow(dead_code)]
        ignored_member: bool,
    }
    
    // Print out message when received over floodsub
    impl NetworkBehaviourEventProcess<FloodsubEvent> for MyBehaviour {
        fn inject_event(&mut self, message: FloodsubEvent) {
            if let FloodsubEvent::Message(message) = message {
                println!("Received: '{:?}' from {:?}", String::from_utf8_lossy(&message.data), message.source);
            }
        }
    }
    
    // Add and remove users from active list when they are updated over mDNS
    impl NetworkBehaviourEventProcess<MdnsEvent> for MyBehaviour {
        fn inject_event(&mut self, event: MdnsEvent) {
            match event {
                MdnsEvent::Discovered(list) =>
                    for (peer, _) in list {
                        self.floodsub.add_node_to_partial_view(peer);
                    }
                MdnsEvent::Expired(list) =>
                    for (peer, _) in list {
                        if !self.mdns.has_node(&peer) {
                            self.floodsub.remove_node_from_partial_view(&peer);
                        }
                    }
            }
        }
    }
    
    // Glob together peers and events into a single swarm
    let mut swarm = {
        let mdns = task::block_on(Mdns::new())?;
        let mut behaviour = MyBehaviour {
            floodsub: Floodsub::new(local_peer_id.clone()),
            mdns,
            ignored_member: false,
        };
        behaviour.floodsub.subscribe(floodsub_topic.clone());
        Swarm::new(transport, behaviour, local_peer_id)
    };

    // Connect to a specificed p2p access point
    if let Some(to_dial) = std::env::args().nth(1) {
        let addr: Multiaddr = to_dial.parse()?;
        Swarm::dial_addr(&mut swarm, addr)?;
        println!("Dialed {:?}", to_dial)
    }
    
    // Read full lines from the terminal
    let mut stdin = io::BufReader::new(io::stdin()).lines();
 
    // Attach the network swarm to whatever port the OS wants
    Swarm::listen_on(&mut swarm, "/ip4/0.0.0.0/tcp/0".parse()?)?;
    
    // Actually do things
    let mut listening = false;
    task::block_on(future::poll_fn(move |cx: &mut Context<'_>| {
        loop {
            match stdin.try_poll_next_unpin(cx)? {
                Poll::Ready(Some(line)) => swarm.floodsub.publish(floodsub_topic.clone(), line.as_bytes()),
                Poll::Ready(None) => panic!("Stdin closed"),
                Poll::Pending => break
            }
        }
        loop {
            match swarm.poll_next_unpin(cx) {
                Poll::Ready(Some(event)) => println!("{:?}", event),
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Pending => {
                    if !listening {
                        for addr in Swarm::listeners(&swarm) {
                            println!("Listening on {:?}", addr);
                            listening = true;
                        }
                    }
                    break
                }
            }
        }
        Poll::Pending
    }))
}