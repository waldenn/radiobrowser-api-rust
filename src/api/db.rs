extern crate chrono;
extern crate xml_writer;

use crate::api::data::StationHistoryCurrent;
use crate::api::data::StationAddResult;
use crate::api::data::Station;
use crate::api::data::StationCheck;
use mysql::QueryResult;
use mysql::Value;
use std;
use std::collections::HashMap;
use crate::thread;
extern crate uuid;
use self::uuid::Uuid;

#[derive(Clone)]
pub struct Connection {
    pool: mysql::Pool,
}

impl Connection {
    const COLUMNS: &'static str =
        "StationID,ChangeUuid,StationUuid,Name,Url,Homepage,Favicon,UrlCache,
    Tags,Country,CountryCode,Subcountry,Language,Votes,
    Date_Format(Creation,'%Y-%m-%d %H:%i:%s') AS CreationFormated,
    Codec,Bitrate,Hls,LastCheckOK,
    LastCheckTime,
    Date_Format(LastCheckTime,'%Y-%m-%d %H:%i:%s') AS LastCheckTimeFormated,
    LastCheckOkTime,
    Date_Format(LastCheckOkTime,'%Y-%m-%d %H:%i:%s') AS LastCheckOkTimeFormated,
    ClickTimestamp,
    Date_Format(ClickTimestamp,'%Y-%m-%d %H:%i:%s') AS ClickTimestampFormated,
    clickcount,ClickTrend";

    const COLUMNS_CHECK: &'static str =
        "CheckID, StationUuid, CheckUuid, Source, Codec, Bitrate, Hls, CheckOK,
    CheckTime,
    Date_Format(CheckTime,'%Y-%m-%d %H:%i:%s') AS CheckTimeFormated,
    UrlCache";

    fn fix_multi_field(value: &str) -> String {
        let values: Vec<String> = value.split(",").map(|v| v.trim().to_lowercase().to_string()).collect();
        values.join(",")
    }

    pub fn add_station_opt(&self, name: Option<String>, url: Option<String>, homepage: Option<String>, favicon: Option<String>,
                        country: Option<String>, countrycode: Option<String>, state: Option<String>, language: Option<String>, tags: Option<String>) -> StationAddResult{
        let query = format!("INSERT INTO Station(Name,Url,Homepage,Favicon,Country,CountryCode,Subcountry,Language,Tags,ChangeUuid,StationUuid, UrlCache) 
                                VALUES(:name, :url, :homepage, :favicon, :country, :countrycode, :state, :language, :tags, :changeuuid, :stationuuid, '')");
        
        if name.is_none(){
            return StationAddResult::new_err("name is empty");
        }
        if url.is_none(){
            return StationAddResult::new_err("url is empty");
        }
        let name = name.unwrap();
        if name.len() > 400{
            return StationAddResult::new_err("name is longer than 400 chars");
        }

        let stationuuid = Uuid::new_v4().to_hyphenated().to_string();
        let changeuuid = Uuid::new_v4().to_hyphenated().to_string();
        let params = params!{
            "name" => name,
            "url" => url.unwrap(),
            "homepage" => homepage.unwrap_or_default(),
            "favicon" => favicon.unwrap_or_default(),
            "country" => country.unwrap_or_default(),
            "countrycode" => countrycode.unwrap_or_default(),
            "state" => state.unwrap_or_default(),
            "language" => Connection::fix_multi_field(&language.unwrap_or_default()),
            "tags" => Connection::fix_multi_field(&tags.unwrap_or_default()),
            "changeuuid" => changeuuid,
            "stationuuid" => stationuuid.clone(),
        };

        let results = self.pool.prep_exec(query, params);
        match results {
            Ok(_) => {
                let backup_result = self.backup_stations_by_uuid(&(vec![stationuuid.clone()]));
                match backup_result {
                    Ok(_) => StationAddResult::new_ok(stationuuid),
                    Err(err) => StationAddResult::new_err(&err.to_string())
                }
            },
            Err(err)=>StationAddResult::new_err(&err.to_string())
        }
    }

    fn backup_stations_by_uuid(&self, stationuuids: &Vec<String>) -> Result<(),Box<dyn std::error::Error>>{
        let mut insert_params: Vec<Value> = vec![];
        let mut insert_query = vec![];
        for stationuuid in stationuuids {
            insert_params.push(stationuuid.into());
            insert_query.push("?");
        }

        let query = format!("INSERT INTO StationHistory(StationID,Name,Url,Homepage,Favicon,Country,CountryCode,SubCountry,Language,Tags,Votes,Creation,StationUuid,ChangeUuid)
                                                 SELECT StationID,Name,Url,Homepage,Favicon,Country,CountryCode,SubCountry,Language,Tags,Votes,Creation,StationUuid,ChangeUuid FROM Station WHERE StationUuid IN ({})", insert_query.join(","));
        let mut stmt = self.pool.prepare(query)?;
        stmt.execute(insert_params)?;
        Ok(())
    }

    pub fn get_checks(&self, stationuuid: Option<String>, checkuuid: Option<String>, seconds: u32) -> Vec<StationCheck> {
        let where_seconds = if seconds > 0 {
            format!(
                "TIMESTAMPDIFF(SECOND,CheckTime,now())<{seconds}",
                seconds = seconds
            )
        } else {
            String::from("")
        };

        let results = match stationuuid {
            Some(uuid) => {
                let where_checkuuid_str = if checkuuid.is_some() {
                    " AND CheckTime>=(SELECT CheckTime FROM StationCheckHistory WHERE ChangeUuid=:checkuuid) AND ChangeUuid<>:checkuuid"
                } else {
                    ""
                };

                let query = format!("SELECT {columns} from StationCheckHistory WHERE StationUuid=? {where_checkuuid} {where_seconds} ORDER BY CheckTime", columns = Connection::COLUMNS_CHECK, where_seconds = where_seconds, where_checkuuid = where_checkuuid_str);
                self.pool.prep_exec(query, (uuid,))
            }
            None => {
                let where_checkuuid_str = if checkuuid.is_some() {
                    " AND CheckTime>=(SELECT CheckTime FROM StationCheck WHERE ChangeUuid=:checkuuid) AND ChangeUuid<>:checkuuid"
                } else {
                    ""
                };

                let query = format!("SELECT {columns} from StationCheck WHERE 1=1 {where_checkuuid} {where_seconds} ORDER BY CheckTime", columns = Connection::COLUMNS_CHECK, where_seconds = where_seconds, where_checkuuid = where_checkuuid_str);
                self.pool.prep_exec(query, ())
            }
        };

        self.get_checks_internal(results)
    }

    pub fn get_stations_by_all(
        &self,
        order: &str,
        reverse: bool,
        hidebroken: bool,
        offset: u32,
        limit: u32,
    ) -> Vec<Station> {
        let order = self.filter_order(order);
        let reverse_string = if reverse { "DESC" } else { "ASC" };
        let hidebroken_string = if hidebroken {
            " WHERE LastCheckOK=TRUE"
        } else {
            ""
        };

        let query: String = format!("SELECT {columns} from Station {hidebroken} ORDER BY {order} {reverse} LIMIT {offset},{limit}",
            columns = Connection::COLUMNS, order = order, reverse = reverse_string,
            hidebroken = hidebroken_string, offset = offset, limit = limit);
        let results = self.pool.prep_exec(query, ());
        self.get_stations(results)
    }

    pub fn filter_order(&self, order: &str) -> &str {
        match order {
            "name" => "Name",
            "url" => "Url",
            "homepage" => "Homepage",
            "favicon" => "Favicon",
            "tags" => "Tags",
            "country" => "Country",
            "state" => "Subcountry",
            "language" => "Language",
            "votes" => "Votes",
            "codec" => "Codec",
            "bitrate" => "Bitrate",
            "lastcheckok" => "LastCheckOK",
            "lastchecktime" => "LastCheckTime",
            "clicktimestamp" => "ClickTimestamp",
            "clickcount" => "clickcount",
            "clicktrend" => "ClickTrend",
            "random" => "RAND()",
            _ => "Name",
        }
    }

    pub fn get_stations_broken(&self, limit: u32) -> Vec<Station> {
        self.get_stations_query(format!(
            "SELECT {columns} from Station WHERE LastCheckOK=FALSE ORDER BY rand() LIMIT {limit}",
            columns = Connection::COLUMNS,
            limit = limit
        ))
    }

    pub fn get_stations_improvable(&self, limit: u32) -> Vec<Station> {
        self.get_stations_query(format!(r#"SELECT {columns} from Station WHERE LastCheckOK=TRUE AND (Tags="" OR Country="") ORDER BY RAND() LIMIT {limit}"#,columns = Connection::COLUMNS, limit = limit))
    }

    pub fn get_stations_deleted(&self, limit: u32, id_str: &str) -> Vec<Station> {
        let id = id_str.parse::<u32>();
        let results = match id {
            Ok(id_number) => {
                let query = format!("SELECT {columns} FROM Station st RIGHT JOIN StationHistory sth ON st.StationID=sth.StationID WHERE st.StationID IS NULL AND sth.StationID=? ORDER BY sth.Creation DESC' {limit}",columns = Connection::COLUMNS, limit = limit);
                self.pool.prep_exec(query, (id_number,))
            }
            _ => {
                let query = format!("SELECT {columns} FROM Station st RIGHT JOIN StationHistory sth ON st.StationID=sth.StationID WHERE st.StationID IS NULL AND sth.StationUuid=? ORDER BY sth.Creation DESC' {limit}",columns = Connection::COLUMNS, limit = limit);
                self.pool.prep_exec(query, (id_str,))
            }
        };
        self.get_stations(results)
    }

    pub fn increase_clicks(&self, ip: &str, station: &Station) -> Result<bool,Box<dyn std::error::Error>> {
        let query = format!(r#"SELECT * FROM StationClick WHERE StationID={id} AND IP="{ip}" AND TIME_TO_SEC(TIMEDIFF(Now(),ClickTimestamp))<24*60*60"#, id = station.id, ip = ip);
        let result = self.pool.prep_exec(query, ())?;

        for resultsingle in result {
            for _ in resultsingle {
                return Ok(false);
            }
        }

        let query2 = format!(
            r#"INSERT INTO StationClick(StationID,IP) VALUES({id},"{ip}")"#,
            id = station.id,
            ip = ip
        );
        let result2 = self.pool.prep_exec(query2, ())?;

        let query3 = format!(
            "UPDATE Station SET ClickTimestamp=NOW() WHERE StationID={id}",
            id = station.id
        );
        let result3 = self.pool.prep_exec(query3, ())?;

        if result2.affected_rows() == 1 && result3.affected_rows() == 1 {
            return Ok(true);
        } else {
            return Ok(false);
        }
    }

    pub fn vote_for_station(&self, ip: &str, station: Option<Station>) -> Result<String, String> {
        match station {
            Some(station) => {
                // delete ipcheck entries after 1 day minutes
                let query_1_delete = format!(r#"DELETE FROM IPVoteCheck WHERE TIME_TO_SEC(TIMEDIFF(Now(),VoteTimestamp))>24*60*60"#);
                let _result_1_delete = self.pool.prep_exec(query_1_delete, ()).unwrap();

                // was there a vote from the ip in the last 1 day?
                let query_2_vote_check = format!(
                    r#"SELECT StationID FROM IPVoteCheck WHERE StationID={id} AND IP="{ip}""#,
                    id = station.id,
                    ip = ip
                );
                let result_2_vote_check = self.pool.prep_exec(query_2_vote_check, ()).unwrap();
                for resultsingle in result_2_vote_check {
                    for _ in resultsingle {
                        // do not allow vote
                        return Err("you are voting for the same station too often".to_string());
                    }
                }

                // add vote entry
                let query_3_insert_votecheck = format!(
                    r#"INSERT INTO IPVoteCheck(IP,StationID) VALUES("{ip}",{id})"#,
                    id = station.id,
                    ip = ip
                );
                let result_3_insert_votecheck =
                    self.pool.prep_exec(query_3_insert_votecheck, ()).unwrap();
                if result_3_insert_votecheck.affected_rows() == 0 {
                    return Err("could not insert vote check".to_string());
                }

                // vote for station
                let query_4_update_votes = format!(
                    "UPDATE Station SET Votes=Votes+1 WHERE StationID={id}",
                    id = station.id
                );
                let result_4_update_votes = self.pool.prep_exec(query_4_update_votes, ()).unwrap();
                if result_4_update_votes.affected_rows() == 1 {
                    Ok("voted for station successfully".to_string())
                } else {
                    Err("could not find station with matching id".to_string())
                }
            }
            _ => Err("could not find station with matching id".to_string()),
        }
    }

    pub fn get_stations_advanced(
        &self,
        name: Option<String>,
        name_exact: bool,
        country: Option<String>,
        country_exact: bool,
        countrycode: Option<String>,
        state: Option<String>,
        state_exact: bool,
        language: Option<String>,
        language_exact: bool,
        tag: Option<String>,
        tag_exact: bool,
        tag_list: Vec<String>,
        bitrate_min: u32,
        bitrate_max: u32,
        order: &str,
        reverse: bool,
        hidebroken: bool,
        offset: u32,
        limit: u32,
    ) -> Vec<Station> {
        let order = self.filter_order(order);
        let reverse_string = if reverse { "DESC" } else { "ASC" };
        let hidebroken_string = if hidebroken {
            " AND LastCheckOK=TRUE"
        } else {
            ""
        };
        let mut query = format!(
            "SELECT {columns} from Station WHERE",
            columns = Connection::COLUMNS
        );
        query.push_str(" Bitrate >= :bitrate_min AND Bitrate <= :bitrate_max");
        if name.is_some() {
            if name_exact {
                query.push_str(" AND Name=:name");
            } else {
                query.push_str(" AND Name LIKE CONCAT('%',:name,'%')");
            }
        }
        if country.is_some() {
            if country_exact {
                query.push_str(" AND Country=:country");
            } else {
                query.push_str(" AND Country LIKE CONCAT('%',:country,'%')");
            }
        }
        if countrycode.is_some() {
            query.push_str(" AND UPPER(CountryCode)=UPPER(:countrycode)");
        }
        if state.is_some() {
            if state_exact {
                query.push_str(" AND Subcountry=:state");
            } else {
                query.push_str(" AND Subcountry LIKE CONCAT('%',:state,'%')");
            }
        }
        if language.is_some() {
            if language_exact {
                query.push_str(" AND ( Language=:language OR Language LIKE CONCAT('%,',:language,',%') OR Language LIKE CONCAT('%,',:language) OR Language LIKE CONCAT(:language,',%'))");
            } else {
                query.push_str(" AND Language LIKE CONCAT('%',:language,'%')");
            }
        }
        if tag.is_some() {
            if tag_exact {
                query.push_str(" AND ( Tags=:tag OR Tags LIKE CONCAT('%,',:tag,',%') OR Tags LIKE CONCAT('%,',:tag) OR Tags LIKE CONCAT(:tag,',%'))");
            } else {
                query.push_str(" AND Tags LIKE CONCAT('%',:tag,'%')");
            }
        }
        let mut params = params!{
            "name" => name.unwrap_or_default(),
            "country" => country.unwrap_or_default(),
            "countrycode" => countrycode.unwrap_or_default(),
            "state" => state.unwrap_or_default(),
            "language" => language.unwrap_or_default(),
            "tag" => tag.unwrap_or_default(),
            "bitrate_min" => bitrate_min,
            "bitrate_max" => bitrate_max,
        };
        let mut i = 0;
        for tag in tag_list {
            if tag_exact {
                query.push_str(&format!(" AND ( Tags=:tag{i} OR Tags LIKE CONCAT('%,',:tag{i},',%') OR Tags LIKE CONCAT('%,',:tag{i}) OR Tags LIKE CONCAT(:tag{i},',%'))",i=i));
            } else {
                query.push_str(&format!(" AND Tags LIKE CONCAT('%',:tag{i},'%')",i=i));
            }
            params.push((format!("tag{i}",i=i), Value::from(tag)));
            i += 1;
        }
        query.push_str(&format!(
            " {hidebroken} ORDER BY {order} {reverse} LIMIT {offset},{limit}",
            order = order,
            reverse = reverse_string,
            hidebroken = hidebroken_string,
            offset = offset,
            limit = limit
        ));
        
        let results = self.pool.prep_exec(
            query,
            params,
        );
        self.get_stations(results)
    }

    pub fn get_stations_deleted_all(&self, limit: u32) -> Vec<Station> {
        self.get_stations_query(format!("SELECT {columns} FROM Station st RIGHT JOIN StationHistory sth ON st.StationID=sth.StationID WHERE st.StationID IS NULL ORDER BY sth.Creation DESC' {limit}",columns = Connection::COLUMNS, limit = limit))
    }

    pub fn get_stations_by_column(
        &self,
        column_name: &str,
        search: String,
        exact: bool,
        order: &str,
        reverse: bool,
        hidebroken: bool,
        offset: u32,
        limit: u32,
    ) -> Vec<Station> {
        let order = self.filter_order(order);
        let reverse_string = if reverse { "DESC" } else { "ASC" };
        let hidebroken_string = if hidebroken {
            " AND LastCheckOK=TRUE"
        } else {
            ""
        };
        let query: String = if exact {
            format!("SELECT {columns} from Station WHERE LOWER({column_name})=? {hidebroken} ORDER BY {order} {reverse} LIMIT {offset},{limit}", columns = Connection::COLUMNS, order = order, reverse = reverse_string, hidebroken = hidebroken_string, offset = offset, limit = limit, column_name = column_name)
        } else {
            format!("SELECT {columns} from Station WHERE LOWER({column_name}) LIKE CONCAT('%',?,'%') {hidebroken} ORDER BY {order} {reverse} LIMIT {offset},{limit}", columns = Connection::COLUMNS, order = order, reverse = reverse_string, hidebroken = hidebroken_string, offset = offset, limit = limit, column_name = column_name)
        };
        let results = self.pool.prep_exec(query, (search.to_lowercase(),));
        self.get_stations(results)
    }

    pub fn get_stations_by_column_multiple(
        &self,
        column_name: &str,
        search: Option<String>,
        exact: bool,
        order: &str,
        reverse: bool,
        hidebroken: bool,
        offset: u32,
        limit: u32,
    ) -> Vec<Station> {
        let order = self.filter_order(order);
        let reverse_string = if reverse { "DESC" } else { "ASC" };
        let hidebroken_string = if hidebroken {
            " AND LastCheckOK=TRUE"
        } else {
            ""
        };
        let query: String = if exact {
            format!(
                r"SELECT {columns} from Station WHERE ({column_name}=?
             OR {column_name} LIKE CONCAT('%,',?,',%')
             OR {column_name} LIKE CONCAT(?,',%')
             OR {column_name} LIKE CONCAT('%,',?))
             {hidebroken} ORDER BY {order} {reverse} LIMIT {offset},{limit}",
                columns = Connection::COLUMNS,
                order = order,
                reverse = reverse_string,
                hidebroken = hidebroken_string,
                offset = offset,
                limit = limit,
                column_name = column_name
            )
        } else {
            format!("SELECT {columns} from Station WHERE {column_name} LIKE CONCAT('%',?,'%') {hidebroken} ORDER BY {order} {reverse} LIMIT {offset},{limit}", columns = Connection::COLUMNS, order = order, reverse = reverse_string, hidebroken = hidebroken_string, offset = offset, limit = limit, column_name = column_name)
        };
        let results = if exact {
            self.pool
                .prep_exec(query, (&search, &search, &search, &search))
        } else {
            self.pool.prep_exec(query, (search,))
        };
        self.get_stations(results)
    }

    pub fn get_station_by_uuid(&self, id_str: &str) -> Vec<Station> {
        let query = format!(
            "SELECT {columns} from Station WHERE StationUuid=? ORDER BY Name",
            columns = Connection::COLUMNS
        );
        let results = self.pool.prep_exec(query, (id_str,));
        self.get_stations(results)
    }

    pub fn get_stations_topvote(&self, limit: u32) -> Vec<Station> {
        let query: String;
        query = format!(
            "SELECT {columns} from Station ORDER BY Votes DESC LIMIT {limit}",
            columns = Connection::COLUMNS,
            limit = limit
        );
        self.get_stations_query(query)
    }

    pub fn get_stations_topclick(&self, limit: u32) -> Vec<Station> {
        let query: String;
        query = format!(
            "SELECT {columns} from Station ORDER BY clickcount DESC LIMIT {limit}",
            columns = Connection::COLUMNS,
            limit = limit
        );
        self.get_stations_query(query)
    }

    pub fn get_stations_lastclick(&self, limit: u32) -> Vec<Station> {
        let query: String;
        query = format!(
            "SELECT {columns} from Station ORDER BY ClickTimestamp DESC LIMIT {limit}",
            columns = Connection::COLUMNS,
            limit = limit
        );
        self.get_stations_query(query)
    }

    pub fn get_stations_lastchange(&self, limit: u32) -> Vec<Station> {
        let query: String;
        query = format!(
            "SELECT {columns} from Station ORDER BY Creation DESC LIMIT {limit}",
            columns = Connection::COLUMNS,
            limit = limit
        );
        self.get_stations_query(query)
    }

    pub fn get_changes(&self, stationuuid: Option<String>, changeuuid: Option<String>) -> Vec<StationHistoryCurrent> {
        let changeuuid_str = if changeuuid.is_some() {
            " AND Creation>=(SELECT Creation FROM StationHistory WHERE ChangeUuid=:changeuuid) AND ChangeUuid<>:changeuuid"
        } else {
            ""
        };

        let stationuuid_str = if stationuuid.is_some() {
            " AND StationUuid=:stationuuid"
        }else{
            ""
        };
        
        let query: String = format!("SELECT StationID,ChangeUuid,
                StationUuid,Name,
                Url,Homepage,
                Favicon,Tags,
                Country,Subcountry,
                CountryCode,
                Language,Votes,
                Date_Format(Creation,'%Y-%m-%d %H:%i:%s') AS CreationFormated,
                Ip from StationHistory WHERE 1=:mynumber {changeuuid_str} {stationuuid} ORDER BY Creation ASC", changeuuid_str = changeuuid_str, stationuuid = stationuuid_str);
        let results = self.pool.prep_exec(query, params! {
            "mynumber" => 1,
            "stationuuid" => stationuuid.unwrap_or(String::from("")),
            "changeuuid" => changeuuid.unwrap_or(String::from(""))
        });
        self.get_stations_history(results)
    }

    fn get_stations_query(&self, query: String) -> Vec<Station> {
        let results = self.pool.prep_exec(query, ());
        self.get_stations(results)
    }

    fn get_stations(&self, results: ::mysql::Result<QueryResult<'static>>) -> Vec<Station> {
        let mut stations: Vec<Station> = vec![];
        for result in results {
            for row_ in result {
                let mut row = row_.unwrap();
                let s = Station::new(
                    row.take("StationID").unwrap(),
                    row.take("ChangeUuid").unwrap_or("".to_string()),
                    row.take("StationUuid").unwrap_or("".to_string()),
                    row.take("Name").unwrap_or("".to_string()),
                    row.take("Url").unwrap_or("".to_string()),
                    row.take("UrlCache").unwrap_or("".to_string()),
                    row
                        .take_opt("Homepage")
                        .unwrap_or(Ok("".to_string()))
                        .unwrap_or("".to_string()),
                    row
                        .take_opt("Favicon")
                        .unwrap_or(Ok("".to_string()))
                        .unwrap_or("".to_string()),
                    row
                        .take_opt("Tags")
                        .unwrap_or(Ok("".to_string()))
                        .unwrap_or("".to_string()),
                    row
                        .take_opt("Country")
                        .unwrap_or(Ok("".to_string()))
                        .unwrap_or("".to_string()),
                    row
                        .take_opt("CountryCode")
                        .unwrap_or(Ok("".to_string()))
                        .unwrap_or("".to_string()),
                    row
                        .take_opt("Subcountry")
                        .unwrap_or(Ok("".to_string()))
                        .unwrap_or("".to_string()),
                    row
                        .take_opt("Language")
                        .unwrap_or(Ok("".to_string()))
                        .unwrap_or("".to_string()),
                    row.take_opt("Votes").unwrap_or(Ok(0)).unwrap_or(0),
                    row
                        .take_opt("CreationFormated")
                        .unwrap_or(Ok("".to_string()))
                        .unwrap_or("".to_string()),
                    row
                        .take_opt("Codec")
                        .unwrap_or(Ok("".to_string()))
                        .unwrap_or("".to_string()),
                    row.take_opt("Bitrate").unwrap_or(Ok(0)).unwrap_or(0),
                    row.take_opt("Hls").unwrap_or(Ok(0)).unwrap_or(0),
                    row.take_opt("LastCheckOK").unwrap_or(Ok(0)).unwrap_or(0),
                    row
                        .take_opt("LastCheckTimeFormated")
                        .unwrap_or(Ok("".to_string()))
                        .unwrap_or("".to_string()),
                    row
                        .take_opt("LastCheckOkTimeFormated")
                        .unwrap_or(Ok("".to_string()))
                        .unwrap_or("".to_string()),
                    row
                        .take_opt("ClickTimestampFormated")
                        .unwrap_or(Ok("".to_string()))
                        .unwrap_or("".to_string()),
                    row.take_opt("clickcount").unwrap_or(Ok(0)).unwrap_or(0),
                    row.take_opt("ClickTrend").unwrap_or(Ok(0)).unwrap_or(0),
                );
                stations.push(s);
            }
        }

        stations
    }

    fn get_stations_history(&self, results: ::mysql::Result<QueryResult<'static>>) -> Vec<StationHistoryCurrent> {
        let mut changes: Vec<StationHistoryCurrent> = vec![];
        for result in results {
            for row_ in result {
                let mut row = row_.unwrap();
                let s = StationHistoryCurrent::new(
                    row.take("StationID").unwrap(),
                    row.take("ChangeUuid").unwrap_or("".to_string()),
                    row.take("StationUuid").unwrap_or("".to_string()),
                    row.take("Name").unwrap_or("".to_string()),
                    row.take("Url").unwrap_or("".to_string()),
                    row
                        .take_opt("Homepage")
                        .unwrap_or(Ok("".to_string()))
                        .unwrap_or("".to_string()),
                    row
                        .take_opt("Favicon")
                        .unwrap_or(Ok("".to_string()))
                        .unwrap_or("".to_string()),
                    row
                        .take_opt("Tags")
                        .unwrap_or(Ok("".to_string()))
                        .unwrap_or("".to_string()),
                    row
                        .take_opt("Country")
                        .unwrap_or(Ok("".to_string()))
                        .unwrap_or("".to_string()),
                    row
                        .take_opt("CountryCode")
                        .unwrap_or(Ok("".to_string()))
                        .unwrap_or("".to_string()),
                    row
                        .take_opt("Subcountry")
                        .unwrap_or(Ok("".to_string()))
                        .unwrap_or("".to_string()),
                    row
                        .take_opt("Language")
                        .unwrap_or(Ok("".to_string()))
                        .unwrap_or("".to_string()),
                    row.take_opt("Votes").unwrap_or(Ok(0)).unwrap_or(0),
                    row
                        .take_opt("CreationFormated")
                        .unwrap_or(Ok("".to_string()))
                        .unwrap_or("".to_string()),
                );
                changes.push(s);
            }
        }

        changes
    }

    fn get_checks_internal(
        &self,
        results: ::mysql::Result<QueryResult<'static>>,
    ) -> Vec<StationCheck> {
        let mut checks: Vec<StationCheck> = vec![];
        for result in results {
            for row_ in result {
                let mut row = row_.unwrap();
                let s = StationCheck::new(
                    row.take("CheckID").unwrap(),
                    row.take("StationUuid").unwrap_or("".to_string()),
                    row.take("CheckUuid").unwrap_or("".to_string()),
                    row.take("Source").unwrap_or("".to_string()),
                    row
                        .take_opt("Codec")
                        .unwrap_or(Ok("".to_string()))
                        .unwrap_or("".to_string()),
                    row.take_opt("Bitrate").unwrap_or(Ok(0)).unwrap_or(0),
                    row.take_opt("Hls").unwrap_or(Ok(0)).unwrap_or(0),
                    row.take_opt("CheckOK").unwrap_or(Ok(0)).unwrap_or(0),
                    row
                        .take_opt("CheckTimeFormated")
                        .unwrap_or(Ok("".to_string()))
                        .unwrap_or("".to_string()),
                    row
                        .take_opt("UrlCache")
                        .unwrap_or(Ok("".to_string()))
                        .unwrap_or("".to_string()),
                );
                checks.push(s);
            }
        }

        checks
    }
}

/// Get currently cached items from table
fn get_cached_items(
    pool: &mysql::Pool,
    table_name: &str,
    column_name: &str,
) -> HashMap<String, (u32, u32)> {
    let mut items = HashMap::new();
    let mut my_stmt = pool
        .prepare(format!(
            "SELECT {column_name},StationCount, StationCountWorking FROM {table_name}",
            table_name = table_name,
            column_name = column_name
        )).unwrap();
    let my_results = my_stmt.execute(());

    for my_result in my_results {
        for my_row in my_result {
            let mut row_unwrapped = my_row.unwrap();
            let key: String = row_unwrapped.take(0).unwrap_or("".into());
            let value: u32 = row_unwrapped.take(1).unwrap_or(0);
            let value_working: u32 = row_unwrapped.take(2).unwrap_or(0);
            let lower = key.to_lowercase();
            items.insert(lower, (value, value_working));
        }
    }
    items
}

/// Get items from a single column from Station table, add number of occurences
/// Supports columns with multiple values that are split by komma
fn get_stations_multi_items(pool: &mysql::Pool, column_name: &str) -> HashMap<String, (u32,u32)> {
    let mut items = HashMap::new();
    let mut my_stmt = pool
        .prepare(format!(
            "SELECT {column_name}, LastCheckOK FROM Station",
            column_name = column_name
        )).unwrap();
    let my_results = my_stmt.execute(());

    for my_result in my_results {
        for my_row in my_result {
            let mut row_unwrapped = my_row.unwrap();
            let tags_str: String = row_unwrapped.take(0).unwrap_or("".into());
            let ok: bool = row_unwrapped.take(1).unwrap_or(false);
            let tags_arr = tags_str.split(',');
            for single_tag in tags_arr {
                let single_tag_trimmed = single_tag.trim().to_lowercase();
                if single_tag_trimmed != "" {
                    let counter = items.entry(single_tag_trimmed).or_insert((0,0));
                    counter.0 += 1;
                    if ok{
                        counter.1 += 1;
                    }
                }
            }
        }
    }
    items
}

fn update_cache_item(
    pool: &mysql::Pool,
    tag: &String,
    count: u32,
    count_working: u32,
    table_name: &str,
    column_name: &str,
) {
    let mut my_stmt = pool
        .prepare(format!(
            r"UPDATE {table_name} SET StationCount=?, StationCountWorking=? WHERE {column_name}=?",
            table_name = table_name,
            column_name = column_name
        )).unwrap();
    let params = (count, count_working, tag);
    let result = my_stmt.execute(params);
    match result {
        Ok(_) => {}
        Err(err) => {
            error!("{}", err);
        }
    }
}

fn insert_to_cache(
    pool: &mysql::Pool,
    tags: HashMap<&String, (u32,u32)>,
    table_name: &str,
    column_name: &str,
) {
    let query = format!(
        "INSERT INTO {table_name}({column_name},StationCount,StationCountWorking) VALUES(?,?,?)",
        table_name = table_name,
        column_name = column_name
    );
    let mut my_stmt = pool.prepare(query.trim_matches(',')).unwrap();
    for item in tags.iter() {
        let result = my_stmt.execute((item.0, (item.1).0, (item.1).1));
        match result {
            Ok(_) => {}
            Err(err) => {
                error!("{}", err);
            }
        }
    }
}

fn remove_from_cache(pool: &mysql::Pool, tags: Vec<&String>, table_name: &str, column_name: &str) {
    let mut query = format!(
        "DELETE FROM {table_name} WHERE {column_name}=''",
        table_name = table_name,
        column_name = column_name
    );
    for _ in 0..tags.len() {
        query.push_str(" OR ");
        query.push_str(column_name);
        query.push_str("=?");
    }
    let mut my_stmt = pool.prepare(query).unwrap();
    let result = my_stmt.execute(tags);
    match result {
        Ok(_) => {}
        Err(err) => {
            error!("{}", err);
        }
    }
}

pub struct RefreshCacheStatus{
    old_items: usize,
    new_items: usize,
    changed_items: usize,
}

pub fn refresh_cache_items(
    pool: &mysql::Pool,
    cache_table_name: &str,
    cache_column_name: &str,
    station_column_name: &str,
)-> RefreshCacheStatus {
    let items_cached = get_cached_items(pool, cache_table_name, cache_column_name);
    let items_current = get_stations_multi_items(pool, station_column_name);
    let mut changed = 0;
    let max_cache_item_len = 110;

    let mut to_delete = vec![];
    for item_cached in items_cached.keys() {
        if !items_current.contains_key(item_cached) {
            to_delete.push(item_cached);
        }
    }
    remove_from_cache(pool, to_delete, cache_table_name, cache_column_name);

    let mut to_insert: HashMap<&String, (u32,u32)> = HashMap::new();
    for item_current in items_current.keys() {
        if !items_cached.contains_key(item_current) {
            if item_current.len() < max_cache_item_len {
                to_insert.insert(item_current, *items_current.get(item_current).unwrap_or(&(0,0)));
            }else{
                warn!("cached '{}' item too long: '{}'", station_column_name, item_current);
            }
        } else {
            let value_new = *items_current.get(item_current).unwrap_or(&(0,0));
            let value_old = *items_cached.get(item_current).unwrap_or(&(0,0));
            if value_old != value_new {
                update_cache_item(
                    pool,
                    item_current,
                    value_new.0,
                    value_new.1,
                    cache_table_name,
                    cache_column_name,
                );
                changed = changed + 1;
            }
        }
    }
    insert_to_cache(pool, to_insert, cache_table_name, cache_column_name);
    trace!(
        "{}: {} -> {}, Changed: {}",
        station_column_name,
        items_cached.len(),
        items_current.len(),
        changed
    );
    RefreshCacheStatus{
        old_items: items_cached.len(),
        new_items: items_current.len(),
        changed_items: changed,
    }
}

pub fn start_refresh_worker(connection_string: String, update_caches_interval: u64) {
    if update_caches_interval > 0 {
        thread::spawn(move || {
            loop {
                let pool = mysql::Pool::new(&connection_string);
                match pool {
                    Ok(p) => {
                        trace!("REFRESH START");
                        let tags = refresh_cache_items(&p, "TagCache", "TagName", "Tags");
                        let languages = refresh_cache_items(&p, "LanguageCache", "LanguageName", "Language");
                        debug!("Refresh(Tags={}->{} changed={}, Languages={}->{} changed={})", tags.old_items, tags.new_items, tags.changed_items, languages.old_items, languages.new_items, languages.changed_items);
                    }
                    Err(e) => error!("{}", e),
                }

                thread::sleep(::std::time::Duration::new(update_caches_interval, 0));
            }
        });
    }
}

pub fn new(connection_string: &String) -> Result<Connection, Box<dyn std::error::Error>> {
    let pool = mysql::Pool::new(connection_string)?;
    Ok(Connection { pool })
}
