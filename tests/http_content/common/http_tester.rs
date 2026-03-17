//! `HttpTester` provides a proxy-based environment for intercepting and asserting
//! HTTP traffic between a driver and alternator.
//!
//! ### Workflow
//! 1. **Initialize**: Call [`HttpTester::start`] with the target alternator address
//!    and a cleanup function.
//! 2. **Configure Driver**: Create a client pointing to [`test.get_proxy_address()`].
//! 3. **Execute & Intercept**:
//!    - Use [`call_with_proxy`] to run driver tasks alongside a proxy interceptor.
//!    - Pass [`proxy::forward_on_request`] to simply forward messages without assertions.
//!    - Use [`call_with_cleanup`] for post-run assertions that don't involve the proxy.
//!    - Both methods automatically trigger [`finish`] if a panic occurs.
//! 4. **Teardown**: Call [`finish`] to stop the proxy and run the cleanup task.
//!
//! ### Example
//! ```rust
//! let test = HttpTester::start(alternator_addr, my_cleanup).await;
//! let client = driver.client(endpoint = test.get_proxy_address());
//!
//! // Driver calls with intercepting proxy
//! test.call_with_proxy(
//!     async { /* driver calls */ },
//!     |request, sender| async {
//!         /* assert and forward requests */
//!     }
//! ).await;
//!
//! // Independent assertions
//! test.call_with_cleanup(async {
//!     /* check final state */
//! }).await;
//!
//! test.finish().await;
//! ```
use crate::http_content::proxy::*;

use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::client::conn::http1::SendRequest;
use hyper::{Request, Response};

use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::Arc;

use futures::FutureExt;

use tokio::select;
use tokio::sync::Mutex;

type ProxyFuture = Pin<Box<dyn Future<Output = ()>>>;

type OnRequestFn = Box<
    dyn Fn(
            Request<Incoming>,
            Arc<Mutex<SendRequest<Full<Bytes>>>>,
        ) -> Pin<Box<dyn Future<Output = Response<Full<Bytes>>> + Send>>
        + Send
        + Sync,
>;

type CleanupFn = Box<dyn Fn(String) -> Pin<Box<dyn Future<Output = ()>>>>;

pub struct HttpTester {
    proxy: Option<ProxyFuture>,
    proxy_address: String,
    alternator_address: String,
    on_request: Arc<Mutex<OnRequestFn>>,
    cleanup_calls: CleanupFn,
}

#[allow(dead_code)]
impl HttpTester {
    pub async fn start<F, Fut>(alternator_address: String, cleanup_calls: F) -> HttpTester
    where
        F: Fn(String) -> Fut + 'static,
        Fut: Future<Output = ()> + 'static,
    {
        // wrap on_request for easy swapping
        let inner_on_request: OnRequestFn =
            Box::new(|request, sender| Box::pin(forward_on_request(request, sender)));
        let inner_on_request = Arc::new(Mutex::new(inner_on_request));
        let inner_on_request_clone = inner_on_request.clone();

        let on_request = move |request, sender| {
            let inner_on_request_clone = inner_on_request_clone.clone();
            async move { (inner_on_request_clone.lock().await)(request, sender).await }
        };

        // start proxy
        let proxy = Proxy::start(
            "localhost:0".to_string(),
            alternator_address.clone(),
            on_request,
            None,
            None,
        )
        .await;

        // construct
        let cleanup_calls: CleanupFn = Box::new(move |s| Box::pin(cleanup_calls(s)));
        let proxy_address = proxy.address().to_string();
        let proxy = Box::pin(proxy.run());

        Self {
            proxy: Some(proxy),
            proxy_address,
            alternator_address,
            on_request: inner_on_request,
            cleanup_calls,
        }
    }

    pub async fn finish(&mut self) {
        if self.proxy.is_some() {
            self.proxy.take();

            let cleanup_result =
                AssertUnwindSafe((self.cleanup_calls)(self.alternator_address.clone()))
                    .catch_unwind()
                    .await;

            if let Err(err) = cleanup_result {
                println!("Could not cleanup made calls: {:?}", err);
            }
        }
    }

    pub fn get_proxy_address(&self) -> &String {
        &self.proxy_address
    }

    pub async fn call_with_proxy<R, Fut>(
        &mut self,
        calls: impl Future<Output = ()> + Send,
        on_request: R,
    ) where
        R: Fn(Request<Incoming>, Arc<Mutex<SendRequest<Full<Bytes>>>>) -> Fut
            + Send
            + Sync
            + 'static,
        Fut: Future<Output = Response<Full<Bytes>>> + Send + 'static,
    {
        if self.proxy.is_none() {
            panic!("HttpTester has finished, cannot perform calls");
        }

        // swap on_request inside proxy
        *self.on_request.lock().await =
            Box::new(move |request, sender| Box::pin(on_request(request, sender)));

        // perform calls while proxy is listening
        let (proxy_finished, result) = select! {
            proxy_result = AssertUnwindSafe(self.proxy.as_mut().unwrap()).catch_unwind().fuse() => (true, proxy_result),
            calls_result = AssertUnwindSafe(calls).catch_unwind().fuse() => (false, calls_result)
        };

        // result & clean up
        if result.is_err() || proxy_finished {
            self.finish().await;

            if proxy_finished {
                result.expect("Proxy panicked during execution");
                panic!("Proxy finished before all calls were made");
            }
            result.expect("Driver calls future panicked during execution");
        }
    }

    pub async fn call_with_cleanup(&mut self, calls: impl Future<Output = ()>) {
        if self.proxy.is_none() {
            panic!("HttpTester has finished, cannot perform calls");
        }

        // perform calls
        let result = AssertUnwindSafe(calls).catch_unwind().await;

        // clean up
        if result.is_err() {
            self.finish().await;
        }
        result.unwrap();
    }
}

impl Drop for HttpTester {
    fn drop(&mut self) {
        if self.proxy.is_some() {
            println!(
                "HttpTester dropped before HttpTester::finish was awaited, cleanup may have been omitted. Consider using HttpTester::finish or HttpTester::call_with_cleanup."
            );
        }
    }
}
