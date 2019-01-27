// #[derive(Default, Debug)]
// pub struct Baz {
//     registry: MetricRegistry,
// }


// impl Baz {

//     pub async fn baz(&self, should_fail: bool) -> Result<(), &'static str> {
//         let reponse_time = &self.registry.baz.reponse_time;
//         measure! { reponse_time, {
//               let delay =
//                   std::time::Duration::from_millis(rand::random::<u64>() % 2000);
//               let when = std::time::Instant::now() + delay;
//               tokio::await!(tokio :: timer :: Delay :: new ( when
//                             )).map_err(|_| "Tokio timer error")?;
//               if !should_fail {
//                 println!("baz !"); 
//                 Ok(()) 
//               } else { 
//                 Err("I failed!") 
                  
//               }
//             }
//         }
//     }

//     pub async fn baz_real(&self, should_fail: bool) -> Result<(), &'static str> {
//         let reponse_time = &self.registry.baz.reponse_time;
//         measure! { reponse_time, {
//               let delay =
//                   std::time::Duration::from_millis(rand::random::<u64>() % 2000);
//               let when = std::time::Instant::now() + delay;
//               tokio::await!(tokio :: timer :: Delay :: new ( when
//                             )).map_err(|_| "Tokio timer error")?;
//               if !should_fail {
//                 println!("baz !"); 
//                 Ok(()) 
//               } else { 
//                 Err("I failed!") 
                  
//               }
//             }
//         }
//     }
// }
// #[derive(Debug, Default)]
// struct MetricRegistry {
//     baz: MetricRegistryBaz,
// }
// #[derive(Debug, Default)]
// struct MetricRegistryBaz {
//     reponse_time: ReponseTime,
// }