#[derive(Clone,Debug)]
pub struct StationItem {
    pub id: i32,
    pub changeuuid: String,
    pub stationuuid: String,
    pub name: String,
    pub url: String,
    pub url_resolved: String,
    pub homepage: String,
    pub favicon: String,
    pub tags: String,
    pub country: String,
    pub countrycode: String,
    pub state: String,
    pub language: String,
    pub votes: i32,
    pub lastchangetime: String,
    pub codec: String,
    pub bitrate: u32,
    pub hls: bool,
    pub lastcheckok: bool,
    pub lastchecktime: String,
    pub lastcheckoktime: String,
    pub lastlocalchecktime: String,
    pub clicktimestamp: String,
    pub clickcount: u32,
    pub clicktrend: i32,
}