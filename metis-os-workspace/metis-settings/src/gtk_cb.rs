//! Shared GTK callback and cache type aliases (keeps clippy type-complexity quiet).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use metis_config::GamingConfig;
use metis_protocol::OutputModeInfo;

pub type Fn0 = Rc<dyn Fn()>;
pub type FnStr = Rc<dyn Fn(&str)>;
pub type FnString = Rc<dyn Fn(String)>;
pub type OptFn0Cell = Rc<RefCell<Option<Fn0>>>;
pub type OptFnStrRef = RefCell<Option<FnStr>>;
pub type OptFnStringRef = RefCell<Option<FnString>>;
pub type BarConfigMutate = Box<dyn FnOnce(&mut metis_config::BarConfig)>;
pub type OptBarConfigMutate = RefCell<Option<BarConfigMutate>>;
pub type OutputModesCache =
    Rc<RefCell<HashMap<String, (Vec<OutputModeInfo>, Option<OutputModeInfo>)>>>;
pub type GamingPersist = Rc<dyn Fn(Box<dyn FnOnce(&mut GamingConfig)>)>;
pub type TabBarHandler = (gtk::Box, FnStr);
